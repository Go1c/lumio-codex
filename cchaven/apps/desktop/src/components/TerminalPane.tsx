import { useCallback, useEffect, useRef, useState } from "react";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import "@xterm/xterm/css/xterm.css";
import { t } from "../i18n";
import { EVENTS, toApiError } from "../lib/api";
import {
  Osc52Filter,
  readLocalClipboard,
  writeLocalClipboard,
} from "../lib/terminalClipboard";
import { useApi } from "../state/ApiProvider";
import { Spinner } from "./ui";

/** Paste in chunks so a large clipboard does not overwhelm the PTY write path. */
const PASTE_CHUNK = 4096;

/** Cursor keys change meaning under DECCKM; tmux and Claude both rely on it. */
const ARROW_SEQUENCES: Record<string, [normal: string, application: string]> = {
  ArrowUp: ["\x1b[A", "\x1bOA"],
  ArrowDown: ["\x1b[B", "\x1bOB"],
  ArrowRight: ["\x1b[C", "\x1bOC"],
  ArrowLeft: ["\x1b[D", "\x1bOD"],
  Home: ["\x1b[H", "\x1bOH"],
  End: ["\x1b[F", "\x1bOF"],
};

/** 6.3 重连策略：指数退避 2s→5s→10s→30s 封顶。 */
export const BACKOFF_SECONDS = [2, 5, 10, 30];

export function backoffFor(attempt: number): number {
  return BACKOFF_SECONDS[Math.min(attempt, BACKOFF_SECONDS.length - 1)];
}

type Phase = "connecting" | "running" | "dropped" | "failed" | "reconnected";

export function TerminalPane({ projectId, host }: { projectId: string; host: string }) {
  const api = useApi();
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<XTerm | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const attemptRef = useRef(0);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const copySelectionRef = useRef<(() => boolean) | null>(null);

  const [phase, setPhase] = useState<Phase>("connecting");
  const [countdown, setCountdown] = useState(0);
  const [error, setError] = useState("");
  const [hint, setHint] = useState("");
  const hintTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const showHint = useCallback((message: string) => {
    setHint(message);
    if (hintTimer.current) clearTimeout(hintTimer.current);
    hintTimer.current = setTimeout(() => setHint(""), 1800);
  }, []);

  const connect = useCallback(async () => {
    setPhase((current) => (current === "dropped" ? current : "connecting"));
    const term = termRef.current;
    try {
      await api.startTerminal(projectId, term?.cols ?? 80, term?.rows ?? 24);
      const reconnecting = attemptRef.current > 0;
      attemptRef.current = 0;
      setPhase(reconnecting ? "reconnected" : "running");
      if (reconnecting) {
        window.setTimeout(
          () => setPhase((current) => (current === "reconnected" ? "running" : current)),
          2000,
        );
      }
    } catch (caught) {
      setError(toApiError(caught).message);
      setPhase("failed");
    }
  }, [api, projectId]);

  /** Schedule an automatic reconnect with backoff; also drives the countdown. */
  const scheduleReconnect = useCallback(() => {
    setPhase("dropped");
    const seconds = backoffFor(attemptRef.current);
    attemptRef.current += 1;
    setCountdown(seconds);

    if (timerRef.current) clearInterval(timerRef.current);
    timerRef.current = setInterval(() => {
      setCountdown((remaining) => {
        if (remaining <= 1) {
          if (timerRef.current) clearInterval(timerRef.current);
          timerRef.current = null;
          void connect();
          return 0;
        }
        return remaining - 1;
      });
    }, 1000);
  }, [connect]);

  const reconnectNow = useCallback(() => {
    if (timerRef.current) {
      clearInterval(timerRef.current);
      timerRef.current = null;
    }
    void api.closeTerminal(projectId).finally(() => void connect());
  }, [api, connect, projectId]);

  useEffect(() => {
    if (!containerRef.current) return undefined;

    const term = new XTerm({
      cursorBlink: true,
      fontSize: 13,
      fontFamily: "Menlo, Monaco, 'Courier New', monospace",
      theme: { background: "#16181d" },
      allowProposedApi: true,
      // Remote apps that grab the mouse (Claude, tmux) would otherwise make
      // local text selection impossible; Option-drag and right-click keep it.
      macOptionClickForcesSelection: true,
      rightClickSelectsWord: true,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(new WebLinksAddon());
    term.open(containerRef.current);
    try {
      fit.fit();
    } catch {
      // jsdom has no layout; the real window fits on the first resize instead.
    }
    termRef.current = term;
    fitRef.current = fit;

    const encoder = new TextEncoder();
    const write = (text: string) =>
      api.writeTerminal(projectId, Array.from(encoder.encode(text)));

    // Remote apps (Claude's "c to copy", tmux) copy with OSC 52. Those bytes
    // must never reach the screen: strip them and put the payload on the Mac's
    // clipboard instead.
    const oscFilter = new Osc52Filter();
    const utf8Decoder = new TextDecoder("utf-8", { fatal: false });
    // DECCKM: `ESC [ ? 1 h` switches cursor keys to application mode.
    let appCursorKeys = false;

    const disposers: Array<() => void> = [];
    void api
      .on<number[]>(EVENTS.terminalOutput(projectId), (payload) => {
        const text = utf8Decoder.decode(new Uint8Array(payload), { stream: true });
        const { display, copies } = oscFilter.push(text);
        for (const copied of copies) {
          void writeLocalClipboard(copied).then(() =>
            showHint(t("terminal.copiedFromRemote")),
          );
        }
        if (!display) return;
        if (display.includes("\x1b[?1h")) appCursorKeys = true;
        if (display.includes("\x1b[?1l")) appCursorKeys = false;
        term.write(display);
      })
      .then((dispose) => disposers.push(dispose));
    void api
      .on(EVENTS.terminalClosed(projectId), () => scheduleReconnect())
      .then((dispose) => disposers.push(dispose));

    const copySelection = () => {
      const selection = term.getSelection();
      if (!selection) {
        showHint(t("terminal.noSelection"));
        return false;
      }
      void writeLocalClipboard(selection).then(() => showHint(t("terminal.copied")));
      return true;
    };

    const pasteFromLocal = async () => {
      const text = await readLocalClipboard();
      if (!text) {
        showHint(t("terminal.clipboardEmpty"));
        return;
      }
      const bytes = Array.from(encoder.encode(text));
      for (let index = 0; index < bytes.length; index += PASTE_CHUNK) {
        await api.writeTerminal(projectId, bytes.slice(index, index + PASTE_CHUNK));
      }
    };

    copySelectionRef.current = copySelection;

    // Only `keydown`: `keyup` would double-fire every intercepted chord.
    term.attachCustomKeyEventHandler((event) => {
      if (event.type !== "keydown") return true;
      const key = event.key.toLowerCase();

      // Cmd+C (or Ctrl+Shift+C) copies the selection locally. Plain Ctrl+C with
      // no selection must still reach the remote as SIGINT.
      if (
        (event.metaKey && !event.ctrlKey && !event.altKey && key === "c") ||
        (event.ctrlKey && event.shiftKey && !event.metaKey && key === "c")
      ) {
        if (term.hasSelection()) {
          event.preventDefault();
          event.stopPropagation();
          copySelection();
          return false;
        }
        if (event.metaKey) {
          event.preventDefault();
          showHint(t("terminal.noSelection"));
          return false;
        }
        return true;
      }

      if (
        (event.metaKey && !event.ctrlKey && !event.altKey && key === "v") ||
        (event.ctrlKey && event.shiftKey && !event.metaKey && key === "v") ||
        (event.shiftKey && event.key === "Insert")
      ) {
        event.preventDefault();
        event.stopPropagation();
        void pasteFromLocal();
        return false;
      }

      // WKWebView swallows bare arrow keys, so send them ourselves in the
      // cursor-key mode the remote actually asked for.
      const arrows = ARROW_SEQUENCES[event.key];
      if (arrows && !event.metaKey && !event.ctrlKey && !event.altKey) {
        event.preventDefault();
        event.stopPropagation();
        void write(appCursorKeys ? arrows[1] : arrows[0]);
        return false;
      }
      return true;
    });

    const input = term.onData((data) => {
      void write(data);
    });
    const resize = term.onResize(({ cols, rows }) => {
      void api.resizeTerminal(projectId, cols, rows);
    });
    const onWindowResize = () => {
      try {
        fit.fit();
      } catch {
        /* ignore */
      }
    };
    window.addEventListener("resize", onWindowResize);

    void connect();

    return () => {
      input.dispose();
      resize.dispose();
      disposers.forEach((dispose) => dispose());
      window.removeEventListener("resize", onWindowResize);
      if (timerRef.current) clearInterval(timerRef.current);
      void api.closeTerminal(projectId);
      term.dispose();
      termRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, projectId]);

  return (
    <div className="term-wrap">
      <div className="term-toolbar">
        <button
          type="button"
          className="btn btn-secondary btn-sm"
          onClick={() => void api.newClaudeSession(projectId)}
        >
          {t("sessions.newSession")}
        </button>
        <button
          type="button"
          className="btn btn-ghost btn-sm"
          onClick={() => void api.listTmuxWindows(projectId)}
        >
          {t("sessions.listWindows")}
        </button>
        <button
          type="button"
          className="btn btn-ghost btn-sm"
          onClick={() => void api.closeTmuxWindow(projectId)}
        >
          {t("sessions.closeWindow")}
        </button>
        <button
          type="button"
          className="btn btn-ghost btn-sm"
          onClick={() => copySelectionRef.current?.()}
        >
          {t("terminal.copySelection")}
        </button>
        <button
          type="button"
          className="btn btn-danger btn-sm"
          onClick={() => {
            if (!window.confirm(t("sessions.killAllConfirm"))) return;
            void api.killAllSessions(projectId);
          }}
        >
          {t("sessions.killAll")}
        </button>
        {hint && (
          <span className="term-hint" role="status">
            {hint}
          </span>
        )}
      </div>

      <div ref={containerRef} className="term-host" data-testid="terminal-host" />

      {phase === "connecting" && (
        <div className="term-overlay">
          <span>
            <Spinner />
            {t("workspace.terminalConnecting", { host })}
          </span>
        </div>
      )}

      {phase === "dropped" && (
        <div className="term-banner" role="alert">
          {t("workspace.terminalDropped", { n: countdown })}
          <button type="button" onClick={reconnectNow}>
            {t("workspace.terminalReconnectNow")}
          </button>
        </div>
      )}

      {phase === "reconnected" && (
        <div className="term-banner ok" role="status">
          {t("workspace.terminalReconnected")}
        </div>
      )}

      {phase === "failed" && (
        <div className="term-overlay">
          <div className="error-card">
            <h4>{t("workspace.terminalFailedTitle")}</h4>
            <p>{error}</p>
            <button type="button" className="btn btn-secondary" onClick={reconnectNow}>
              {t("common.retry")}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
