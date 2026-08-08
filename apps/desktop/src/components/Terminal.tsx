import { useEffect, useRef } from "react";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
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

  useEffect(() => {
    if (!containerRef.current) return;

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
      // Alt+Backspace should delete a word (common macOS expectation).
      altClickMovesCursor: true,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(new WebLinksAddon());
    term.open(containerRef.current);
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

    // Listen for PTY output.
    let unlistenOutput: UnlistenFn | undefined;
    listen<number[]>(`terminal-output-${projectId}`, (event) => {
      const data = new Uint8Array(event.payload);
      term.write(data);
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
    const writeProxy = (data: Parameters<XTerm["write"]>[0]) => {
      if (typeof data === "string") {
        if (data.includes("\x1b[?1h")) appCursorKeys = true;
        if (data.includes("\x1b[?1l")) appCursorKeys = false;
      }
      origWrite(data);
    };
    term.write = writeProxy;

    const arrowMap: Record<string, [string, string]> = {
      ArrowUp: ["\x1b[A", "\x1bOA"],
      ArrowDown: ["\x1b[B", "\x1bOB"],
      ArrowRight: ["\x1b[C", "\x1bOC"],
      ArrowLeft: ["\x1b[D", "\x1bOD"],
      Home: ["\x1b[H", "\x1bOH"],
      End: ["\x1b[F", "\x1bOF"],
    };

    // Intercept arrow keys at the KeyboardEvent level to prevent the
    // WKWebView from swallowing them, and manually send the correct
    // escape sequence based on the current cursor-key mode.
    // CRITICAL: only handle 'keydown' — keyup fires for the same key and
    // would cause double input.
    // Returns false → xterm.js skips its own processing (no onData).
    const keyHandler = (ev: KeyboardEvent) => {
      if (ev.type !== "keydown") return true;
      const arrows = arrowMap[ev.key];
      if (arrows) {
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
      invoke("close_terminal", { projectId }).catch(() => {});
      term.dispose();
    };
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
