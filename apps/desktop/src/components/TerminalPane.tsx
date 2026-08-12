import { useCallback, useEffect, useRef, useState } from "react";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import "@xterm/xterm/css/xterm.css";
import { t } from "../i18n";
import { EVENTS, toApiError } from "../lib/api";
import { useApi } from "../state/ApiProvider";
import { Spinner } from "./ui";

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

  const [phase, setPhase] = useState<Phase>("connecting");
  const [countdown, setCountdown] = useState(0);
  const [error, setError] = useState("");

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

    const disposers: Array<() => void> = [];
    void api
      .on<number[]>(EVENTS.terminalOutput(projectId), (payload) => {
        term.write(new Uint8Array(payload));
      })
      .then((dispose) => disposers.push(dispose));
    void api
      .on(EVENTS.terminalClosed(projectId), () => scheduleReconnect())
      .then((dispose) => disposers.push(dispose));

    const input = term.onData((data) => {
      void api.writeTerminal(projectId, Array.from(new TextEncoder().encode(data)));
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
