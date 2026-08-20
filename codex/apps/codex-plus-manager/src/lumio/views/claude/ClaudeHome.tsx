import { useEffect, type ReactNode } from "react";

import { refreshClaudeConflicts, refreshClaudeFiles, resumeClaudeSync } from "../../claude/session.ts";
import { dispatchClaude } from "../../claude/store.ts";
import type { ClaudeState } from "../../claude/types.ts";
import { ConflictsPane } from "./ConflictsPane.tsx";
import { FileExplorer } from "./FileExplorer.tsx";
import { ProjectRail } from "./ProjectRail.tsx";
import { StatusBar } from "./StatusBar.tsx";
import { ServerStatusPane, SessionsPane } from "./StatusDrawer.tsx";
import { TerminalPane } from "./TerminalPane.tsx";

const EMPTY_FILES: ClaudeState["filesByProject"][string] = [];
const EMPTY_CONFLICTS: ClaudeState["conflictsByProject"][string] = [];

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
  const files = active ? (state.filesByProject[active.id] ?? EMPTY_FILES) : EMPTY_FILES;
  const conflicts = active ? (state.conflictsByProject[active.id] ?? EMPTY_CONFLICTS) : EMPTY_CONFLICTS;

  useEffect(() => {
    if (active) void resumeClaudeSync(active.id);
  }, [active?.id]);

  useEffect(() => {
    if (active && state.stageTab === "files") void refreshClaudeFiles(active.id);
    if (active && state.stageTab === "conflicts") void refreshClaudeConflicts(active.id);
  }, [active?.id, state.stageTab]);

  return (
    <div className="lumio-claude-frame">
      <ProjectRail active={active} onConnect={onConnect} ordersSlot={ordersSlot} state={state} />
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
          <button
            className={state.stageTab === "server" ? "is-on" : ""}
            onClick={() => dispatchClaude({ type: "set-stage-tab", tab: "server" })}
            type="button"
          >服务器状态</button>
          <button
            className={state.stageTab === "sessions" ? "is-on" : ""}
            onClick={() => dispatchClaude({ type: "set-stage-tab", tab: "sessions" })}
            type="button"
          >对话状态</button>
        </nav>
        {active === null ? (
          <div className="lumio-claude-term">
            <div className="dim">还没有项目</div>
          </div>
        ) : (
          <>
            <TerminalPane hidden={state.stageTab !== "terminal"} project={active} />
            {state.stageTab === "files" ? <FileExplorer files={files} project={active} /> : null}
            {state.stageTab === "conflicts" ? (
              <ConflictsPane conflicts={conflicts} projectId={active.id} />
            ) : null}
            {state.stageTab === "server" ? <ServerStatusPane projectId={active.id} /> : null}
            {state.stageTab === "sessions" ? <SessionsPane projectId={active.id} /> : null}
          </>
        )}
        <StatusBar active={active} sync={sync} />
      </section>
    </div>
  );
}
