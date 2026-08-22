import { type ReactNode } from "react";

import {
  groupProjectsByHost,
  isServerGroupOpen,
  shouldShowServerShell,
} from "../../claude/rail-groups.ts";
import { resumeClaudeSync } from "../../claude/session.ts";
import { dispatchClaude } from "../../claude/store.ts";
import { SYNC_RELAUNCH_LABEL, isSyncCaughtUp } from "../../claude/sync-status.ts";
import { serverMetaCopy } from "./status-copy.ts";
import type {
  ClaudeChatSession,
  ClaudeCliInstallStatus,
  ClaudeLoginStatus,
  ClaudeProject,
  ClaudeState,
  ClaudeSyncStatus,
} from "../../claude/types.ts";

export type ProjectRailProps = {
  projects: ClaudeProject[];
  activeProjectId: string | null;
  syncByProject: ClaudeState["syncByProject"];
  sessionsByProject: ClaudeState["sessionsByProject"];
  collapsedHosts: Record<string, boolean>;
  cliByHost: Record<string, ClaudeCliInstallStatus>;
  loginByHost: Record<string, ClaudeLoginStatus>;
  onlineHosts?: Record<string, boolean>;
  onSelectProject: (projectId: string) => void;
  onNewProject: (host: string) => void;
  onConnectServer: () => void;
};

type ProjectRailLegacyProps = {
  state: ClaudeState;
  active: ClaudeProject | null;
  onConnect: () => void;
  ordersSlot?: ReactNode;
};

export function ProjectRail(props: ProjectRailProps): ReactNode;
export function ProjectRail(props: ProjectRailLegacyProps): ReactNode;
export function ProjectRail(props: ProjectRailProps | ProjectRailLegacyProps): ReactNode {
  if (isLegacyProps(props)) {
    const activeId = props.active?.id ?? props.state.activeProjectId;
    return (
      <ProjectRailView
        projects={props.state.projects}
        activeProjectId={activeId}
        syncByProject={props.state.syncByProject}
        sessionsByProject={props.state.sessionsByProject}
        collapsedHosts={props.state.collapsedHosts}
        cliByHost={props.state.cliByHost}
        loginByHost={props.state.loginByHost}
        onlineHosts={onlineHostsFromState(props.state)}
        onSelectProject={(projectId) => {
          dispatchClaude({ type: "select-project", projectId });
          void resumeClaudeSync(projectId);
        }}
        onNewProject={() => props.onConnect()}
        onConnectServer={props.onConnect}
      />
    );
  }
  return <ProjectRailView {...props} />;
}

function isLegacyProps(props: ProjectRailProps | ProjectRailLegacyProps): props is ProjectRailLegacyProps {
  return "state" in props;
}

export function onlineHostsFromState(state: ClaudeState): Record<string, boolean> {
  const onlineHosts: Record<string, boolean> = {};
  for (const project of state.projects) {
    const phase = state.workspacePhaseByProject[project.id];
    const sync = state.syncByProject[project.id];
    const online = phase !== "offline" && sync?.state !== "offline";
    onlineHosts[project.host] = Boolean(onlineHosts[project.host]) || online;
  }
  return onlineHosts;
}

function hostIsOnline(host: string, onlineHosts?: Record<string, boolean>): boolean {
  if (onlineHosts && Object.hasOwn(onlineHosts, host)) return onlineHosts[host] === true;
  return true;
}

function liveCount(sessions: ClaudeChatSession[] | undefined): number {
  if (!sessions) return 0;
  let n = 0;
  for (const session of sessions) {
    if (session.running) n += 1;
  }
  return n;
}

function ChevronIcon() {
  return (
    <svg
      viewBox="0 0 16 16"
      width={11}
      height={11}
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M6 3.5 10.5 8 6 12.5" />
    </svg>
  );
}

function FolderIcon() {
  return (
    <svg
      viewBox="0 0 16 16"
      width={13}
      height={13}
      fill="none"
      stroke="currentColor"
      strokeWidth="1.2"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M1.6 4.3c0-.7.5-1.2 1.2-1.2h2.5c.4 0 .7.1.9.4l.8 1h5.2c.7 0 1.2.5 1.2 1.2v6.1c0 .7-.5 1.2-1.2 1.2H2.8c-.7 0-1.2-.5-1.2-1.2V4.3Z" />
    </svg>
  );
}

function PlusIcon() {
  return (
    <svg
      viewBox="0 0 16 16"
      width={13}
      height={13}
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      aria-hidden="true"
    >
      <path d="M8 3.6v8.8M3.6 8h8.8" />
    </svg>
  );
}

/* --- server-meta --- */
function serverMetaLine(
  cli: ClaudeCliInstallStatus | undefined,
  login: ClaudeLoginStatus | undefined,
  online: boolean,
): string {
  return serverMetaCopy(cli, login, online);
}

/* --- project-row --- */
function projectMetaLine(
  sync: ClaudeSyncStatus | undefined,
  online: boolean,
): { text: string; tone: "ok" | "warn" | "bad" | "" } {
  if (!online) {
    return { text: sync?.state && sync.state !== "idle" ? "未连接" : "未连接 · 点一下就连", tone: "" };
  }
  if (sync?.state === "conflicts" && sync.conflicts > 0) {
    return { text: `${sync.conflicts} 个冲突待处理`, tone: "warn" };
  }
  if (sync?.state === "fail") return { text: "同步没能完成", tone: "bad" };
  if (sync?.state === "running" && isSyncCaughtUp(sync)) return { text: "已同步", tone: "ok" };
  if (sync?.state === "running") return { text: "正在同步", tone: "" };
  if (sync?.state === "synced") return { text: "已同步", tone: "ok" };
  if (sync?.state === "offline") return { text: "未连接", tone: "" };
  return { text: "未连接 · 点一下就连", tone: "" };
}

function ProjectRailView({
  projects,
  activeProjectId,
  syncByProject,
  sessionsByProject,
  collapsedHosts,
  cliByHost,
  loginByHost,
  onlineHosts,
  onSelectProject,
  onNewProject,
  onConnectServer,
}: ProjectRailProps) {
  const groups = groupProjectsByHost(projects);
  const serverCount = groups.length;
  const showShell = shouldShowServerShell(serverCount);

  return (
    <aside className="lumio-claude-rail">
      <div className="lumio-claude-rail-head">
        <h2>服务器与项目</h2>
      </div>
      <div className="lumio-claude-rail-body">
        {groups.map((group) => {
          const online = hostIsOnline(group.host, onlineHosts);
          const holdsActiveProject = group.projects.some((project) => project.id === activeProjectId);
          const override = Object.hasOwn(collapsedHosts, group.host)
            ? collapsedHosts[group.host]
            : undefined;
          const open = isServerGroupOpen({
            host: group.host,
            serverCount,
            online,
            holdsActiveProject,
            collapsed: override,
          });
          return (
            <section className="lumio-claude-srv-group" key={group.host}>
              {showShell ? (
                <button
                  aria-expanded={open}
                  className={`lumio-claude-srv${open ? " is-open" : ""}${online ? " is-on" : ""}`}
                  onClick={() => {
                    dispatchClaude({ type: "toggle-server-group", host: group.host, collapsed: open });
                  }}
                  type="button"
                >
                  <span className="chev">
                    <ChevronIcon />
                  </span>
                  <i className={`lumio-claude-dot${online ? " is-ok" : ""}`} aria-hidden="true" />
                  <span className="host">{group.host}</span>
                  <span className="meta">{serverMetaLine(cliByHost[group.host], loginByHost[group.host], online)}</span>
                </button>
              ) : null}
              {open ? (
                <div className="lumio-claude-srv-body">
                  {group.projects.map((project) => {
                    const live = liveCount(sessionsByProject[project.id]);
                    const status = projectMetaLine(syncByProject[project.id], online);
                    return (
                      <div className="lumio-claude-proj-wrap" key={project.id}>
                        <button
                          className={`lumio-claude-proj${project.id === activeProjectId ? " is-on" : ""}`}
                          onClick={() => onSelectProject(project.id)}
                          type="button"
                        >
                          <span className="k">
                            <span className="glyph">
                              <FolderIcon />
                            </span>
                            {project.name}
                            {live > 0 ? (
                              <i
                                className="lumio-claude-live"
                                title={`有 ${live} 个对话在跑`}
                                aria-label={`有 ${live} 个对话在跑`}
                              />
                            ) : null}
                          </span>
                          <span className="dir">{project.remoteRoot}</span>
                          <span className={`st${status.tone ? ` is-${status.tone}` : ""}`}>{status.text}</span>
                        </button>
                        {status.tone === "bad" ? (
                          <button
                            className="lumio-claude-proj-recover"
                            onClick={() => void resumeClaudeSync(project.id)}
                            type="button"
                          >
                            {SYNC_RELAUNCH_LABEL}
                          </button>
                        ) : null}
                      </div>
                    );
                  })}
                  <button className="lumio-claude-newproj" onClick={() => onNewProject(group.host)} type="button">
                    <span className="glyph">
                      <PlusIcon />
                    </span>
                    新建项目
                  </button>
                </div>
              ) : null}
            </section>
          );
        })}
      </div>
      <div className="lumio-claude-rail-foot">
        <button className="lumio-claude-connect" onClick={onConnectServer} type="button">
          <span className="glyph">
            <PlusIcon />
          </span>
          连接新服务器
        </button>
      </div>
    </aside>
  );
}
