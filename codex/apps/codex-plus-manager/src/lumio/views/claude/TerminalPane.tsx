import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { Terminal as XTerm } from "@xterm/xterm";
import { useEffect, useRef, useState, type MouseEvent } from "react";
import "@xterm/xterm/css/xterm.css";

import {
  resizeClaudeTerminal,
  startClaudeTerminal,
  subscribeClaudeEvent,
  terminalClosedEvent,
  terminalOutputEvent,
  writeClaudeTerminal,
} from "../../claude/api.ts";
import { projectPassword } from "../../claude/store.ts";
import {
  copyTextForKey,
  firstOpenableHttpsUrl,
  isClaudeLoginUrl,
  terminalContextActions,
  textForClipboard,
} from "../../claude/terminal-clipboard.ts";
import { openInBrowser } from "../../invoke.ts";
import type { ClaudeProject } from "../../claude/types.ts";

export function TerminalPane({
  project,
  hidden,
}: {
  project: ClaudeProject;
  hidden: boolean;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<XTerm | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const [status, setStatus] = useState("正在打开终端…");
  const [loginUrl, setLoginUrl] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    copyText: string | null;
    openUrl: string | null;
  } | null>(null);

  useEffect(() => {
    if (!containerRef.current) return undefined;
    const term = new XTerm({
      cursorBlink: true,
      fontSize: 13,
      fontFamily: "Menlo, Monaco, 'Courier New', monospace",
      rightClickSelectsWord: false,
      theme: { background: "#0c0d11", foreground: "#e8e8ed" },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(
      new WebLinksAddon((_event, uri) => {
        void openInBrowser(uri);
      }),
    );
    term.open(containerRef.current);
    try {
      fit.fit();
    } catch {
      /* jsdom has no layout */
    }
    termRef.current = term;
    fitRef.current = fit;

    const encoder = new TextEncoder();
    term.onData((data) => {
      void writeClaudeTerminal(project.id, Array.from(encoder.encode(data)));
    });
    term.attachCustomKeyEventHandler((event) => {
      if (event.type !== "keydown") return true;
      const text = copyTextForKey({
        key: event.key,
        metaKey: event.metaKey,
        ctrlKey: event.ctrlKey,
        shiftKey: event.shiftKey,
        altKey: event.altKey,
        selection: term.getSelection(),
        visibleText: readVisibleTerminalText(term),
      });
      if (!text) return true;
      event.preventDefault();
      void copyTerminalText(text, setCopied);
      return false;
    });

    const host = term.element;
    const onNativeCopy = (event: ClipboardEvent) => {
      const text = textForClipboard(term.getSelection(), readVisibleTerminalText(term));
      if (!text || !event.clipboardData) return;
      event.clipboardData.setData("text/plain", text);
      event.preventDefault();
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    };
    host?.addEventListener("copy", onNativeCopy);

    const disposers: Array<() => void> = [];
    if (host) {
      disposers.push(() => host.removeEventListener("copy", onNativeCopy));
    }
    const decoder = new TextDecoder("utf-8", { fatal: false });
    void subscribeClaudeEvent<number[]>(terminalOutputEvent(project.id), (payload) => {
      term.write(decoder.decode(new Uint8Array(payload), { stream: true }));
      const visible = readVisibleTerminalText(term);
      const url = firstOpenableHttpsUrl(visible);
      setLoginUrl(url && isClaudeLoginUrl(url) ? url : null);
    }).then((stop) => disposers.push(stop));
    void subscribeClaudeEvent(terminalClosedEvent(project.id), () => {
      setStatus("连接已断开，正在重连…");
      void attach();
    }).then((stop) => disposers.push(stop));

    const attach = async () => {
      try {
        await startClaudeTerminal({
          projectId: project.id,
          host: project.host,
          user: project.user,
          port: project.port,
          password: projectPassword(project.id),
          keyPath: project.keyPath,
          hostAlias: project.hostAlias,
          auth: project.auth,
          remoteRoot: project.remoteRoot,
          cols: term.cols || 80,
          rows: term.rows || 24,
        });
        setStatus("");
      } catch {
        setStatus("没能打开终端。");
      }
    };
    void attach();

    const onResize = () => {
      try {
        fit.fit();
        void resizeClaudeTerminal(project.id, term.cols || 80, term.rows || 24);
      } catch {
        /* ignore */
      }
    };
    window.addEventListener("resize", onResize);

    return () => {
      window.removeEventListener("resize", onResize);
      for (const stop of disposers) stop();
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
  }, [project.id]);

  useEffect(() => {
    if (hidden) return;
    try {
      fitRef.current?.fit();
    } catch {
      /* ignore */
    }
  }, [hidden]);

  const closeMenu = () => setMenu(null);

  const onContextMenu = (event: MouseEvent<HTMLDivElement>) => {
    event.preventDefault();
    const term = termRef.current;
    if (!term) return;
    const actions = terminalContextActions(term.getSelection(), readVisibleTerminalText(term));
    if (!actions.copyText && !actions.openUrl) return;
    const wrap = wrapRef.current?.getBoundingClientRect();
    setMenu({
      x: event.clientX - (wrap?.left ?? 0),
      y: event.clientY - (wrap?.top ?? 0),
      copyText: actions.copyText,
      openUrl: actions.openUrl,
    });
  };

  return (
    <div
      className="lumio-claude-xterm-wrap"
      hidden={hidden}
      onContextMenu={onContextMenu}
      onClick={closeMenu}
      ref={wrapRef}
    >
      {status ? <p className="dim lumio-claude-xterm-status">{status}</p> : null}
      {loginUrl ? (
        <div className="lumio-claude-term-actions">
          <button
            className="lumio-button is-secondary"
            onClick={(event) => {
              event.stopPropagation();
              void copyTerminalText(loginUrl, setCopied);
            }}
            type="button"
          >
            {copied ? "已复制" : "复制登录链接"}
          </button>
          <button
            className="lumio-button"
            onClick={(event) => {
              event.stopPropagation();
              void openInBrowser(loginUrl);
            }}
            type="button"
          >
            用浏览器打开
          </button>
        </div>
      ) : null}
      <div className="lumio-claude-xterm" ref={containerRef} />
      {menu ? (
        <div
          className="lumio-claude-term-menu"
          role="menu"
          style={{ left: menu.x, top: menu.y }}
          onClick={(event) => event.stopPropagation()}
        >
          <button
            disabled={!menu.copyText}
            onClick={() => {
              if (menu.copyText) void copyTerminalText(menu.copyText, setCopied);
              closeMenu();
            }}
            type="button"
          >
            复制
          </button>
          {menu.openUrl ? (
            <button
              onClick={() => {
                void openInBrowser(menu.openUrl ?? "");
                closeMenu();
              }}
              type="button"
            >
              用浏览器打开
            </button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function readVisibleTerminalText(term: XTerm): string {
  const buf = term.buffer.active;
  const start = Math.max(0, buf.viewportY - 8);
  const end = buf.viewportY + term.rows;
  const lines: string[] = [];
  for (let y = start; y < end; y += 1) {
    lines.push(buf.getLine(y)?.translateToString(true) ?? "");
  }
  return lines.join("\n");
}

function fallbackCopy(text: string): void {
  const area = document.createElement("textarea");
  area.value = text;
  area.setAttribute("readonly", "");
  area.style.position = "fixed";
  area.style.left = "-9999px";
  document.body.appendChild(area);
  area.select();
  document.execCommand("copy");
  area.remove();
}

async function copyTerminalText(text: string, setCopied: (value: boolean) => void): Promise<void> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
    } else {
      fallbackCopy(text);
    }
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1600);
  } catch {
    try {
      fallbackCopy(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      setCopied(false);
    }
  }
}
