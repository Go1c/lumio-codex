import { useState } from "react";
import { t } from "../i18n";
import { truncateMiddle } from "../lib/format";
import type {
  ExternalLinks,
  ProjectConfig,
  SessionView,
  SyncState,
  SyncStatus,
} from "../lib/types";
import { AccountMenu, entitlementChipLine, isExpiringSoon } from "./AccountMenu";
import { ContextMenu, StatusDot, type MenuItem } from "./ui";

/** 6.3 状态条文案 — the single source of wording for every sync state. */
export function syncBarLabel(status: SyncStatus, retryInSeconds = 5): string {
  switch (status.state) {
    case "syncing":
      return t("status.syncing", { n: status.pending });
    case "conflicts":
      return t("status.conflicts", { n: status.conflicts });
    case "offline":
      return t("status.offline", { n: retryInSeconds });
    default:
      return t("status.synced");
  }
}

export interface SidebarProps {
  projects: ProjectConfig[];
  statuses: Record<string, SyncStatus>;
  activeProjectId: string | null;
  session: SessionView | null;
  links: ExternalLinks;
  offline: boolean;
  activity: string[];
  onSelectProject: (projectId: string) => void;
  onNewProject: () => void;
  onEditProject: (project: ProjectConfig) => void;
  onRevealProject: (project: ProjectConfig) => void;
  onDeleteProject: (project: ProjectConfig) => void;
  onOpenExternal: (url: string) => void;
  onLogout: () => void;
  onSyncBarClick: (status: SyncStatus) => void;
}

export function Sidebar(props: SidebarProps) {
  const [menu, setMenu] = useState<{ x: number; y: number; project: ProjectConfig } | null>(null);
  const [accountOpen, setAccountOpen] = useState(false);
  const [activityOpen, setActivityOpen] = useState(false);

  const globalStatus = aggregateStatus(props.projects, props.statuses, props.offline);
  const menuItems: MenuItem[] = menu
    ? [
        { label: t("sidebar.editProject"), onSelect: () => props.onEditProject(menu.project) },
        { label: t("common.revealInFinder"), onSelect: () => props.onRevealProject(menu.project) },
        {
          label: t("sidebar.deleteProject"),
          onSelect: () => props.onDeleteProject(menu.project),
          danger: true,
          separatorBefore: true,
        },
      ]
    : [];

  return (
    <aside className="app-sidebar">
      <h2>{t("sidebar.projects")}</h2>
      {props.projects.length === 0 && (
        <div style={{ padding: "4px 10px", fontSize: 13, color: "#8b8f98" }}>
          {t("sidebar.empty")}
        </div>
      )}
      <ul className="proj-list">
        {props.projects.map((project) => {
          const status = props.statuses[project.id]?.state ?? ("offline" as SyncState);
          return (
            <li key={project.id} className="proj-row">
              <button
                type="button"
                className={`proj-item ${props.activeProjectId === project.id ? "active" : ""}`}
                aria-label={project.name}
                aria-current={props.activeProjectId === project.id}
                onClick={() => props.onSelectProject(project.id)}
                onContextMenu={(event) => {
                  event.preventDefault();
                  setMenu({ x: event.clientX, y: event.clientY, project });
                }}
              >
                <StatusDot state={status} />
                <span className="name" title={project.name}>
                  {truncateMiddle(project.name, 22)}
                </span>
              </button>
              <button
                type="button"
                className="menu-btn"
                aria-label={`${project.name} 的更多操作`}
                onClick={(event) => {
                  event.stopPropagation();
                  const rect = event.currentTarget.getBoundingClientRect();
                  setMenu({ x: rect.left - 140, y: rect.bottom + 4, project });
                }}
              >
                …
              </button>
            </li>
          );
        })}
      </ul>

      <button
        type="button"
        className="btn btn-primary"
        style={{ marginTop: 10 }}
        onClick={props.onNewProject}
      >
        {t("sidebar.newProject")}
      </button>

      <div className="sidebar-bottom">
        {activityOpen && (
          <div className="activity-panel">
            <strong style={{ color: "#d7d9de" }}>{t("sidebar.recentActivity")}</strong>
            {props.activity.length === 0 ? (
              <span>{t("sidebar.noActivity")}</span>
            ) : (
              props.activity.map((entry, index) => <span key={`${entry}-${index}`}>{entry}</span>)
            )}
          </div>
        )}
        <button
          type="button"
          className="sync-bar"
          onClick={() => {
            if (globalStatus.state === "conflicts") props.onSyncBarClick(globalStatus);
            else setActivityOpen((open) => !open);
          }}
        >
          <StatusDot state={globalStatus.state} />
          {syncBarLabel(globalStatus, globalStatus.retryInSeconds ?? 5)}
        </button>

        <div style={{ position: "relative" }}>
          {accountOpen && props.session && (
            <AccountMenu
              session={props.session}
              links={props.links}
              onOpenExternal={props.onOpenExternal}
              onLogout={props.onLogout}
              onClose={() => setAccountOpen(false)}
            />
          )}
          <button
            type="button"
            className="account-chip"
            aria-haspopup="menu"
            aria-expanded={accountOpen}
            onClick={() => setAccountOpen((open) => !open)}
          >
            <span className="avatar" aria-hidden="true">
              {props.session?.email.slice(0, 1).toUpperCase() ?? "U"}
            </span>
            <span style={{ minWidth: 0, lineHeight: 1.3 }}>
              <span className="email" title={props.session?.email}>
                {truncateMiddle(props.session?.email ?? "", 20)}
              </span>
              <span
                className={
                  isExpiringSoon(props.session?.entitlement) ? "plan warn" : "plan"
                }
                style={{ display: "block" }}
              >
                {entitlementChipLine(props.session?.entitlement)}
              </span>
            </span>
          </button>
        </div>
      </div>

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={menuItems}
          label={`${menu.project.name} 的更多操作`}
          onClose={() => setMenu(null)}
        />
      )}
    </aside>
  );
}

/** Worst-case wins: conflicts beat offline beat syncing beat synced. */
export function aggregateStatus(
  projects: ProjectConfig[],
  statuses: Record<string, SyncStatus>,
  offline: boolean,
): SyncStatus {
  const conflicts = projects.reduce(
    (total, project) => total + (statuses[project.id]?.conflicts ?? 0),
    0,
  );
  const pending = projects.reduce(
    (total, project) => total + (statuses[project.id]?.pending ?? 0),
    0,
  );
  if (conflicts > 0) return { state: "conflicts", conflicts, pending };
  if (offline || projects.some((project) => statuses[project.id]?.state === "offline")) {
    const retryInSeconds = projects
      .map((project) => statuses[project.id]?.retryInSeconds)
      .filter((value): value is number => typeof value === "number")
      .reduce<number | undefined>(
        (min, value) => (min === undefined ? value : Math.min(min, value)),
        undefined,
      );
    return { state: "offline", conflicts: 0, pending, retryInSeconds };
  }
  if (pending > 0) return { state: "syncing", conflicts: 0, pending };
  return { state: "synced", conflicts: 0, pending: 0 };
}
