import { useEffect, useRef, useState } from "react";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  Osc52Filter,
  readLocalClipboard,
  writeLocalClipboard,
} from "../lib/terminalClipboard";
import "@xterm/xterm/css/xterm.css";

interface TerminalProps {
  projectId: string;
  sshHostAlias: string;
  remoteRoot: string;
  tmuxSession: string;
}

// Pre-allocate a TextEncoder for efficient string→bytes conversion.
// This correctly handles multi-byte UTF-8 (e.g. CJK input) and all
// escape sequences (arrow keys, function keys, etc.).
const encoder = new TextEncoder();

export default function TerminalPane({
  projectId,
  sshHostAlias,
  remoteRoot,
  tmuxSession,
}: TerminalProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<XTerm | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const [copyHint, setCopyHint] = useState<string | null>(null);
  const hintTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const showCopyHint = (msg: string) => {
    setCopyHint(msg);
    if (hintTimerRef.current) clearTimeout(hintTimerRef.current);
    hintTimerRef.current = setTimeout(() => setCopyHint(null), 2000);
  };

  useEffect(() => {
    const host = containerRef.current;
    if (!host) return;

    const term = new XTerm({
      cursorBlink: true,
      fontSize: 14,
      fontFamily:
        "'SF Mono', 'JetBrains Mono', 'Fira Code', Menlo, Monaco, 'Courier New', monospace",
      allowProposedApi: true,
      fontWeight: "normal",
      fontWeightBold: "bold",
      letterSpacing: 0,
      lineHeight: 1.0,
      // macOS: treat Option key as Meta (enables Meta shortcuts in terminal apps).
      macOptionIsMeta: true,
      // Option+click forces local selection even when remote mouse mode is on
      // (Claude TUI / tmux). This is the fix for "can't select on the left".
      macOptionClickForcesSelection: true,
      // Right-click selects the word under the cursor (then user can Cmd+C).
      rightClickSelectsWord: true,
      // Alt+Backspace should delete a word (common macOS expectation).
      altClickMovesCursor: true,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(new WebLinksAddon());
    term.open(host);
    fit.fit();

    termRef.current = term;
    fitRef.current = fit;

    const cols = term.cols;
    const rows = term.rows;

    // Start the terminal session.
    invoke("start_terminal", {
      request: {
        projectId,
        sshHostAlias,
        remoteRoot,
        tmuxSession,
        cols,
        rows,
      },
    }).catch((e) => {
      term.writeln(`\x1b[31mFailed to start terminal: ${e}\x1b[0m`);
    });

    // --- Clipboard bridge -------------------------------------------------
    // 1) OSC 52 from remote apps (Claude "c to copy") → local clipboard
    // 2) Selection Cmd+C / toolbar → local clipboard
    // 3) Cmd+V → local clipboard → PTY (already worked via onData for most
    //    cases; we also handle it explicitly so it never goes to the remote
    //    as a literal control chord).
    const oscFilter = new Osc52Filter();
    // Stream UTF-8 decoder so multi-byte chars split across PTY chunks survive.
    const utf8Decoder = new TextDecoder("utf-8", { fatal: false });

    const applyRemoteCopy = (text: string) => {
      if (!text) return;
      void writeLocalClipboard(text).then(() => {
        showCopyHint("Copied to local clipboard");
      });
    };

    const copySelection = () => {
      const selection = term.getSelection();
      if (!selection) {
        showCopyHint("No selection");
        return false;
      }
      void writeLocalClipboard(selection).then(() => {
        showCopyHint("Copied selection");
      });
      return true;
    };

    const pasteFromLocal = async () => {
      const text = await readLocalClipboard();
      if (!text) {
        showCopyHint("Clipboard empty");
        return;
      }
      const bytes = Array.from(encoder.encode(text));
      // Chunk large pastes so we do not overwhelm the PTY write path.
      const CHUNK = 4096;
      for (let i = 0; i < bytes.length; i += CHUNK) {
        await invoke("write_terminal", {
          projectId,
          data: bytes.slice(i, i + CHUNK),
        });
      }
    };

    // Expose copySelection for toolbar button via ref-stable wrapper.
    (term as XTerm & { __copySelection?: () => boolean }).__copySelection =
      copySelection;

    // Listen for PTY output — strip OSC 52, then render the rest.
    let unlistenOutput: UnlistenFn | undefined;
    listen<number[]>(`terminal-output-${projectId}`, (event) => {
      const data = new Uint8Array(event.payload);
      const text = utf8Decoder.decode(data, { stream: true });
      const { display, copies } = oscFilter.push(text);
      for (const payload of copies) {
        applyRemoteCopy(payload);
      }
      if (display) {
        // Prefer string write so our writeProxy (cursor-key tracking) still sees
        // the text form of DECCKM sequences when they arrive as text.
        term.write(display);
      }
    }).then((fn) => {
      unlistenOutput = fn;
    });

    // Listen for terminal close.
    let unlistenClosed: UnlistenFn | undefined;
    listen(`terminal-closed-${projectId}`, () => {
      term.writeln("\r\n\x1b[33m[Connection closed]\x1b[0m");
    }).then((fn) => {
      unlistenClosed = fn;
    });

    // Forward user input to PTY.
    // Uses TextEncoder for correct UTF-8 encoding of all characters.
    // Arrow keys are handled separately by attachCustomKeyEventHandler below;
    // onData will NOT fire for keys where the custom handler returns false.
    const arrowSeqs = new Set([
      "\x1b[A", "\x1b[B", "\x1b[C", "\x1b[D",   // normal mode
      "\x1bOA", "\x1bOB", "\x1bOC", "\x1bOD",   // app mode
      "\x1b[H", "\x1b[F", "\x1bOH", "\x1bOF",   // home/end
    ]);
    const inputDisposable = term.onData((data) => {
      // Skip arrow key sequences — they are sent by the custom key handler.
      if (arrowSeqs.has(data)) return;
      const bytes = Array.from(encoder.encode(data));
      invoke("write_terminal", {
        projectId,
        data: bytes,
      });
    });

    // Track cursor-key mode (DECCKM) from terminal output to send the
    // correct arrow key sequences:
    //   \x1b[?1h  → application cursor keys (\x1bOA etc.)
    //   \x1b[?1l  → normal cursor keys (\x1b[A etc.)
    let appCursorKeys = false;
    const origWrite = term.write.bind(term);
    const writeProxy = (data: Parameters<XTerm["write"]>[0], callback?: () => void) => {
      if (typeof data === "string") {
        if (data.includes("\x1b[?1h")) appCursorKeys = true;
        if (data.includes("\x1b[?1l")) appCursorKeys = false;
      }
      return origWrite(data, callback);
    };
    term.write = writeProxy as typeof term.write;

    const arrowMap: Record<string, [string, string]> = {
      ArrowUp: ["\x1b[A", "\x1bOA"],
      ArrowDown: ["\x1b[B", "\x1bOB"],
      ArrowRight: ["\x1b[C", "\x1bOC"],
      ArrowLeft: ["\x1b[D", "\x1bOD"],
      Home: ["\x1b[H", "\x1bOH"],
      End: ["\x1b[F", "\x1bOF"],
    };

    // Intercept keys at the KeyboardEvent level:
    // - Arrow keys: prevent WKWebView swallow + correct DECCKM sequence
    // - Cmd/Ctrl+C with selection: copy to local clipboard (not SIGINT to remote)
    // - Cmd/Ctrl+V / Shift+Insert: paste from local clipboard into PTY
    // CRITICAL: only handle 'keydown' — keyup would double-fire.
    // Returns false → xterm.js skips its own processing (no onData).
    const keyHandler = (ev: KeyboardEvent) => {
      if (ev.type !== "keydown") return true;

      const meta = ev.metaKey || ev.ctrlKey;

      // Copy: Cmd+C / Ctrl+Shift+C when there is a selection.
      // Plain Ctrl+C without selection still goes to the remote (SIGINT).
      if (
        (ev.metaKey && !ev.ctrlKey && !ev.altKey && ev.key.toLowerCase() === "c") ||
        (ev.ctrlKey && ev.shiftKey && !ev.metaKey && ev.key.toLowerCase() === "c")
      ) {
        if (term.hasSelection()) {
          ev.preventDefault();
          ev.stopPropagation();
          copySelection();
          return false;
        }
        // No selection: let Ctrl+C fall through as remote interrupt; on macOS
        // Cmd+C with no selection is a no-op (do not send 0x03 via Meta+C).
        if (ev.metaKey) {
          ev.preventDefault();
          showCopyHint("No selection");
          return false;
        }
        return true;
      }

      // Paste: Cmd+V / Ctrl+Shift+V / Shift+Insert
      if (
        (ev.metaKey && !ev.ctrlKey && !ev.altKey && ev.key.toLowerCase() === "v") ||
        (ev.ctrlKey && ev.shiftKey && !ev.metaKey && ev.key.toLowerCase() === "v") ||
        (ev.shiftKey && ev.key === "Insert")
      ) {
        ev.preventDefault();
        ev.stopPropagation();
        void pasteFromLocal();
        return false;
      }

      const arrows = arrowMap[ev.key];
      if (arrows) {
        // Do not steal Option+Arrow for selection modifiers — still send arrows.
        if (meta || ev.altKey) return true;
        ev.preventDefault();
        ev.stopPropagation();
        const seq = appCursorKeys ? arrows[1] : arrows[0];
        const bytes = Array.from(encoder.encode(seq));
        invoke("write_terminal", { projectId, data: bytes });
        return false; // prevent xterm.js from also handling it
      }
      return true;
    };
    term.attachCustomKeyEventHandler(keyHandler);

    // Context-menu copy when the user right-clicks a selection.
    const onContextMenu = () => {
      if (!term.hasSelection()) return;
      // Keep the browser menu for "Copy"; also eagerly sync selection so
      // system copy works if the menu's copy is used.
      const selection = term.getSelection();
      if (selection) {
        void writeLocalClipboard(selection);
      }
    };
    host.addEventListener("contextmenu", onContextMenu);

    // Handle resize.
    const resizeDisposable = term.onResize(({ cols, rows }) => {
      invoke("resize_terminal", { projectId, cols, rows });
    });

    // Window resize handler.
    const handleResize = () => {
      fit.fit();
    };
    window.addEventListener("resize", handleResize);

    return () => {
      inputDisposable.dispose();
      term.write = origWrite;
      resizeDisposable.dispose();
      unlistenOutput?.();
      unlistenClosed?.();
      window.removeEventListener("resize", handleResize);
      host.removeEventListener("contextmenu", onContextMenu);
      if (hintTimerRef.current) clearTimeout(hintTimerRef.current);
      // Flush any incomplete OSC so we do not leak decoder state.
      oscFilter.flush();
      utf8Decoder.decode(new Uint8Array(), { stream: false });
      invoke("close_terminal", { projectId }).catch(() => {});
      term.dispose();
    };
    // showCopyHint is a stable setState wrapper for session-local toast only.
    // eslint-disable-next-line react-hooks/exhaustive-deps -- remount on session identity only
  }, [projectId, sshHostAlias, remoteRoot, tmuxSession]);

  // --- Toolbar actions ---

  const newClaudeSession = () => {
    invoke("new_claude_session", { projectId }).catch(() => {});
  };

  const closeTmuxWindow = () => {
    invoke("close_tmux_window", { projectId }).catch(() => {});
  };

  const listWindows = () => {
    invoke("list_tmux_windows", { projectId }).catch(() => {});
  };

  const killAll = () => {
    if (!confirm("Kill ALL tmux sessions and Claude processes? This cannot be undone.")) return;
    invoke("kill_all_sessions", { projectId }).catch(() => {});
  };

  const copySelectionClick = () => {
    const term = termRef.current;
    if (!term) return;
    const selection = term.getSelection();
    if (!selection) {
      showCopyHint("No selection — hold Option and drag to select");
      return;
    }
    void writeLocalClipboard(selection).then(() => {
      showCopyHint("Copied selection");
    });
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      {/* Terminal toolbar */}
      <div
        style={{
          display: "flex",
          gap: "4px",
          padding: "4px 8px",
          background: "#2d2d2d",
          borderBottom: "1px solid #1a1a1a",
          flexShrink: 0,
          alignItems: "center",
        }}
      >
        <button
          onClick={newClaudeSession}
          style={{
            padding: "4px 12px",
            fontSize: "12px",
            background: "#2563eb",
            color: "white",
            border: "none",
            borderRadius: "4px",
            cursor: "pointer",
          }}
          title="Open a new tmux window and start Claude Code"
        >
          + New Claude
        </button>
        <button
          onClick={listWindows}
          style={{
            padding: "4px 12px",
            fontSize: "12px",
            background: "#404040",
            color: "#ccc",
            border: "none",
            borderRadius: "4px",
            cursor: "pointer",
          }}
          title="List tmux windows (Ctrl-B w)"
        >
          Windows
        </button>
        <button
          onClick={closeTmuxWindow}
          style={{
            padding: "4px 12px",
            fontSize: "12px",
            background: "#dc2626",
            color: "white",
            border: "none",
            borderRadius: "4px",
            cursor: "pointer",
          }}
          title="Close current tmux window (Ctrl-B &)"
        >
          Close Window
        </button>
        <button
          onClick={copySelectionClick}
          style={{
            padding: "4px 12px",
            fontSize: "12px",
            background: "#404040",
            color: "#ccc",
            border: "none",
            borderRadius: "4px",
            cursor: "pointer",
          }}
          title="Copy selection to local clipboard (Cmd+C). Hold Option and drag to select when remote mouse mode is on."
        >
          Copy
        </button>
        {copyHint ? (
          <span
            style={{
              marginLeft: "8px",
              fontSize: "11px",
              color: "#86efac",
            }}
          >
            {copyHint}
          </span>
        ) : (
          <span
            style={{
              marginLeft: "8px",
              fontSize: "11px",
              color: "#888",
            }}
          >
            Option+drag to select · Cmd+C copy · Cmd+V paste
          </span>
        )}
        <button
          onClick={killAll}
          style={{
            padding: "4px 12px",
            fontSize: "12px",
            background: "#991b1b",
            color: "white",
            border: "none",
            borderRadius: "4px",
            cursor: "pointer",
            marginLeft: "auto",
          }}
          title="Kill ALL tmux sessions and Claude processes (Ctrl-B :kill-server)"
        >
          ☠ Kill All
        </button>
      </div>
      {/* Terminal container */}
      <div
        ref={containerRef}
        style={{ flex: 1, background: "#1e1e1e", padding: "4px" }}
      />
    </div>
  );
}
