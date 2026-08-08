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
      fontFamily: "Menlo, Monaco, 'Courier New', monospace",
      allowProposedApi: true,
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
    const inputDisposable = term.onData((data) => {
      invoke("write_terminal", {
        projectId,
        data: Array.from(data).map((c) => c.charCodeAt(0)),
      });
    });

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
      resizeDisposable.dispose();
      unlistenOutput?.();
      unlistenClosed?.();
      window.removeEventListener("resize", handleResize);
      invoke("close_terminal", { projectId }).catch(() => {});
      term.dispose();
    };
  }, [projectId, sshHostAlias, remoteRoot, tmuxSession]);

  return (
    <div
      ref={containerRef}
      style={{ width: "100%", height: "100%", background: "#1e1e1e", padding: "4px" }}
    />
  );
}
