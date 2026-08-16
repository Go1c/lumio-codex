import { type FormEvent, useEffect, useState } from "react";

import {
  openProjectSystemTerminal,
  refreshClaudeFiles,
  runProjectCommand,
} from "../../claude/session.ts";
import { dispatchClaude } from "../../claude/store.ts";
import type { ClaudeProject, ClaudeState } from "../../claude/types.ts";

export function ClaudeHome({
  state,
  onConnect,
}: {
  state: ClaudeState;
  onConnect: () => void;
}) {
  const active =
    state.projects.find((project) => project.id === state.activeProjectId) ?? state.projects[0] ?? null;
  const sync = active ? state.syncByProject[active.id] : null;
  const lines = active ? (state.terminalByProject[active.id] ?? []) : [];
  const files = active ? (state.filesByProject[active.id] ?? []) : [];
  const conflicts = active ? (state.conflictsByProject[active.id] ?? []) : [];

  useEffect(() => {
    if (active && state.stageTab === "files") void refreshClaudeFiles(active.id);
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
        ) : state.stageTab === "files" ? (
          <FilesPane files={files} project={active} />
        ) : state.stageTab === "conflicts" ? (
          <ConflictsPane conflicts={conflicts} />
        ) : (
          <TerminalPane lines={lines} project={active} />
        )}
        <div className="lumio-claude-status">
          <span>
            {sync?.state === "conflicts"
              ? `${sync.conflicts} 个冲突`
              : sync?.state === "synced"
                ? "已同步 · 文件与远端一致"
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
  lines,
}: {
  project: ClaudeProject;
  lines: ClaudeState["terminalByProject"][string];
}) {
  const [command, setCommand] = useState("");
  const [opening, setOpening] = useState(false);

  const onRun = (event: FormEvent) => {
    event.preventDefault();
    const next = command;
    setCommand("");
    void runProjectCommand(project.id, next);
  };

  return (
    <div className="lumio-claude-term">
      {lines.map((line, index) => (
        <div className={line.kind} key={`${index}-${line.kind}`}>
          {line.text}
        </div>
      ))}
      <p className="dim">
        应用内还没有交互式终端。可以在下面跑一条远程命令，或打开系统终端连过去。
      </p>
      <form className="lumio-claude-term-form" onSubmit={onRun}>
        <span className="dim">&gt;</span>
        <input
          aria-label="远程命令"
          onChange={(event) => setCommand(event.target.value)}
          placeholder="uname -a"
          value={command}
        />
        <button className="lumio-button is-secondary" type="submit">
          运行
        </button>
        <button
          className="lumio-button is-secondary"
          disabled={opening}
          onClick={() => {
            setOpening(true);
            void openProjectSystemTerminal(project.id).finally(() => setOpening(false));
          }}
          type="button"
        >
          {opening ? "正在打开…" : "打开系统终端"}
        </button>
      </form>
    </div>
  );
}

function FilesPane({
  project,
  files,
}: {
  project: ClaudeProject;
  files: ClaudeState["filesByProject"][string];
}) {
  return (
    <div className="lumio-claude-files">
      <p className="dim">本机 {project.localRoot}</p>
      {files.length === 0 ? (
        <p>还没有同步下来的文件。完整文件树下一步再接。</p>
      ) : (
        <ul>
          {files.map((file) => (
            <li key={file.path}>
              <span>{file.kind === "directory" ? "📁" : "📄"}</span>
              {file.name}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function ConflictsPane({
  conflicts,
}: {
  conflicts: ClaudeState["conflictsByProject"][string];
}) {
  return (
    <div className="lumio-claude-files">
      {conflicts.length === 0 ? (
        <p>暂无冲突。远端和本机的改动不会被静默覆盖。</p>
      ) : (
        <ul>
          {conflicts.map((conflict) => (
            <li key={conflict.id}>
              <strong>{conflict.path}</strong>
              <span className="dim">{conflict.kindLabel}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
