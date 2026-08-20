import { type ReactNode } from "react";

import { resumeClaudeSync } from "../../claude/session.ts";
import { dispatchClaude } from "../../claude/store.ts";
import type { ClaudeProject, ClaudeState } from "../../claude/types.ts";
import { ClaudeEntitlementLine } from "./ClaudeEntitlementLine.tsx";

export function ProjectRail({
  state,
  active,
  onConnect,
  ordersSlot,
}: {
  state: ClaudeState;
  active: ClaudeProject | null;
  onConnect: () => void;
  ordersSlot?: ReactNode;
}) {
  return (
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
          onClick={() => {
            dispatchClaude({ type: "select-project", projectId: project.id });
            void resumeClaudeSync(project.id);
          }}
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
