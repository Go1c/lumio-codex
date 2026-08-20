import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { Terminal as XTerm } from "@xterm/xterm";
import { useEffect, useRef, useState, type MouseEvent, type ReactNode } from "react";
import "@xterm/xterm/css/xterm.css";

import {
  previewClaudeFile,
  resizeClaudeTerminal,
  startClaudeTerminal,
  subscribeClaudeEvent,
  terminalClosedEvent,
  terminalOutputEvent,
  writeClaudeTerminal,
} from "../../claude/api.ts";
import {
  refreshClaudeConflicts,
  refreshClaudeFiles,
  resolveProjectConflict,
} from "../../claude/session.ts";
import { dispatchClaude } from "../../claude/store.ts";
import { projectPassword } from "../../claude/store.ts";
import {
  copyTextForKey,
  firstOpenableHttpsUrl,
  isClaudeLoginUrl,
  terminalContextActions,
  textForClipboard,
} from "../../claude/terminal-clipboard.ts";
import { openInBrowser } from "../../invoke.ts";
import type {
  ClaudeConflictEntry,
  ClaudeConflictResolution,
  ClaudeFileEntry,
  ClaudeFilePreview,
  ClaudeProject,
  ClaudeState,
} from "../../claude/types.ts";
import { ClaudeEntitlementLine } from "./ClaudeEntitlementLine.tsx";

export function ClaudeHome({
  state,
  onConnect,
  ordersSlot,
}: {
  state: ClaudeState;
  onConnect: () => void;
  ordersSlot?: ReactNode;
}) {
  const active =
    state.projects.find((project) => project.id === state.activeProjectId) ?? state.projects[0] ?? null;
  const sync = active ? state.syncByProject[active.id] : null;
  const files = active ? (state.filesByProject[active.id] ?? []) : [];
  const conflicts = active ? (state.conflictsByProject[active.id] ?? []) : [];

  useEffect(() => {
    if (active && state.stageTab === "files") void refreshClaudeFiles(active.id);
    if (active && state.stageTab === "conflicts") void refreshClaudeConflicts(active.id);
  }, [active?.id, state.stageTab]);

  return (
    <div className="lumio-claude-frame">
      <aside className="lumio-claude-rail">
        <div className="lumio-claude-rail-head">
          <h2>项目</h2>
          <button className="lumio-button is-secondary" onClick={onConnect} type="button">
            新建
          </button>
        </div>
        <ClaudeEntitlementLine entitlement={state.entitlement} />
        {state.projects.map((project) => (
          <button
            className={`lumio-claude-proj${project.id === active?.id ? " is-on" : ""}`}
            key={project.id}
            onClick={() => dispatchClaude({ type: "select-project", projectId: project.id })}
            type="button"
          >
            <span className="k">{project.name}</span>
            <span className="d">{projectSummary(project, state)}</span>
          </button>
        ))}
        {ordersSlot}
        <button className="lumio-button is-secondary lumio-claude-add" onClick={onConnect} type="button">
          连接新服务器
        </button>
      </aside>
      <section className="lumio-claude-stage">
        <nav className="lumio-claude-stage-tabs" aria-label="工作台">
          <button
            className={state.stageTab === "terminal" ? "is-on" : ""}
            onClick={() => dispatchClaude({ type: "set-stage-tab", tab: "terminal" })}
            type="button"
          >终端</button>
          <button
            className={state.stageTab === "files" ? "is-on" : ""}
            onClick={() => dispatchClaude({ type: "set-stage-tab", tab: "files" })}
            type="button"
          >文件</button>
          <button
            className={state.stageTab === "conflicts" ? "is-on" : ""}
            onClick={() => dispatchClaude({ type: "set-stage-tab", tab: "conflicts" })}
            type="button"
          >冲突</button>
        </nav>
        {active === null ? (
          <div className="lumio-claude-term">
            <div className="dim">还没有项目</div>
          </div>
        ) : (
          <>
            <TerminalPane hidden={state.stageTab !== "terminal"} project={active} />
            {state.stageTab === "files" ? <FilesPane files={files} project={active} /> : null}
            {state.stageTab === "conflicts" ? (
              <ConflictsPane conflicts={conflicts} projectId={active.id} />
            ) : null}
          </>
        )}
        <div className="lumio-claude-status">
          <span>
            {sync?.state === "conflicts"
              ? `${sync.conflicts} 个冲突`
              : sync?.state === "synced"
                ? "已同步 · 文件与远端一致"
                : sync?.state === "running"
                  ? `正在同步 ${sync.filesDone} / ${sync.filesTotal || "…"}`
                  : sync?.state === "offline"
                    ? "离线 · 本机目录可用"
                    : "本机目录已就绪"}
          </span>
          <span>{active ? `${active.user}@${active.host}` : ""}</span>
        </div>
      </section>
    </div>
  );
}

function projectSummary(project: ClaudeProject, state: ClaudeState): string {
  const sync = state.syncByProject[project.id];
  if (sync?.state === "conflicts" && sync.conflicts > 0) {
    return `${sync.conflicts} 个冲突 · ${project.host}`;
  }
  if (sync?.state === "synced") return `已同步 · ${project.host}`;
  return project.host;
}

function TerminalPane({
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

function FilesPane({
  project,
  files,
}: {
  project: ClaudeProject;
  files: ClaudeState["filesByProject"][string];
}) {
  const [preview, setPreview] = useState<ClaudeFilePreview | null>(null);
  const local = files.filter((file) => file.side !== "remote");
  const remote = files.filter((file) => file.side === "remote");

  const openFile = (file: ClaudeFileEntry) => {
    if (file.kind === "directory") return;
    void previewClaudeFile({
      host: project.host,
      user: project.user,
      port: project.port,
      password: projectPassword(project.id),
      keyPath: project.keyPath,
      hostAlias: project.hostAlias,
      auth: project.auth,
      localRoot: project.localRoot,
      remoteRoot: project.remoteRoot,
      path: file.path,
      side: file.side === "remote" ? "remote" : "local",
    }).then(setPreview);
  };

  return (
    <div className="lumio-claude-files">
      <div className="lumio-claude-files-split">
        <div>
          <p className="dim">本机 {project.localRoot}</p>
          <FileTree files={local} onOpen={openFile} />
        </div>
        <div>
          <p className="dim">远端 {project.remoteRoot}</p>
          <FileTree files={remote} onOpen={openFile} />
        </div>
      </div>
      {preview ? (
        <pre className="lumio-claude-preview">
          {preview.tooLarge
            ? "文件太大，没法在这里预览。"
            : preview.binary
              ? "这是二进制文件，没法预览。"
              : preview.content}
        </pre>
      ) : null}
    </div>
  );
}

function FileTree({
  files,
  onOpen,
}: {
  files: ClaudeFileEntry[];
  onOpen: (file: ClaudeFileEntry) => void;
}) {
  if (files.length === 0) {
    return <p>还没有同步下来的文件。</p>;
  }
  return (
    <ul>
      {files.map((file) => (
        <li key={`${file.side ?? "local"}:${file.path}`}>
          <button onClick={() => onOpen(file)} type="button">
            {file.kind === "directory" ? `${file.name}/` : file.name}
          </button>
          {file.children && file.children.length > 0 ? (
            <FileTree files={file.children} onOpen={onOpen} />
          ) : null}
        </li>
      ))}
    </ul>
  );
}

function ConflictsPane({
  projectId,
  conflicts,
}: {
  projectId: string;
  conflicts: ClaudeState["conflictsByProject"][string];
}) {
  const [selectedId, setSelectedId] = useState<string | null>(conflicts[0]?.id ?? null);
  const current = conflicts.find((item) => item.id === selectedId) ?? conflicts[0] ?? null;

  useEffect(() => {
    if (!conflicts.some((item) => item.id === selectedId)) {
      setSelectedId(conflicts[0]?.id ?? null);
    }
  }, [conflicts, selectedId]);

  const resolve = (resolution: ClaudeConflictResolution) => {
    if (!current) return;
    void resolveProjectConflict(projectId, current.id, resolution);
  };

  if (conflicts.length === 0) {
    return (
      <div className="lumio-claude-files">
        <p>暂无冲突。远端和本机的改动不会被静默覆盖。</p>
      </div>
    );
  }

  return (
    <div className="lumio-claude-conflicts">
      <ul className="lumio-claude-conflict-list">
        {conflicts.map((conflict) => (
          <li key={conflict.id}>
            <button
              className={current?.id === conflict.id ? "is-on" : ""}
              onClick={() => setSelectedId(conflict.id)}
              type="button"
            >
              <strong>{conflict.path}</strong>
              <span className="dim">{conflict.kindLabel}</span>
            </button>
          </li>
        ))}
      </ul>
      {current ? (
        <div className="lumio-claude-conflict-detail">
          <div className="lumio-claude-conflict-actions">
            <button className="lumio-button is-secondary" onClick={() => resolve("keepLocal")} type="button">
              保留本地
            </button>
            <button className="lumio-button is-secondary" onClick={() => resolve("keepRemote")} type="button">
              保留远端
            </button>
            <button className="lumio-button is-secondary" onClick={() => resolve("keepBoth")} type="button">
              两者都保留
            </button>
          </div>
          <div className="lumio-claude-diff">
            <pre>
              <span className="dim">本地</span>
              {"\n"}
              {current.localContent ?? ""}
            </pre>
            <pre>
              <span className="dim">远端</span>
              {"\n"}
              {current.remoteContent ?? ""}
            </pre>
          </div>
        </div>
      ) : null}
    </div>
  );
}
