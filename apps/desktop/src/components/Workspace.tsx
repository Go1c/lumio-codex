import { useCallback, useEffect, useState } from "react";
import { t } from "../i18n";
import { toApiError } from "../lib/api";
import { useApi } from "../state/ApiProvider";
import { ConflictsView } from "./ConflictsView";
import { FilesExplorer } from "./FilesExplorer";
import { TerminalPane } from "./TerminalPane";
import { StatusDot } from "./ui";
import type { Conflict, ProjectConfig, SyncStatus } from "../lib/types";

export type WorkspaceTab = "terminal" | "files" | "conflicts";

/** 5.5 工作区：顶部信息栏 + 三个 Tab。 */
export function Workspace({
  project,
  status,
  offline,
  onStatusChanged,
}: {
  project: ProjectConfig;
  status: SyncStatus;
  offline: boolean;
  onStatusChanged: () => void | Promise<void>;
}) {
  const api = useApi();
  const [tab, setTab] = useState<WorkspaceTab>("terminal");
  const [conflicts, setConflicts] = useState<Conflict[]>([]);
  const [error, setError] = useState("");

  const loadConflicts = useCallback(async () => {
    try {
      setConflicts(await api.listConflicts(project.id));
    } catch (caught) {
      setError(toApiError(caught).message);
    }
  }, [api, project.id]);

  useEffect(() => {
    setTab("terminal");
    void loadConflicts();
  }, [loadConflicts]);

  const tabs: Array<[WorkspaceTab, string]> = [
    ["terminal", t("workspace.tabTerminal")],
    ["files", t("workspace.tabFiles")],
    ["conflicts", t("workspace.tabConflicts")],
  ];

  const connectionLabel = offline
    ? t("workspace.disconnected")
    : t("workspace.connected", {
        sync: status.state === "conflicts" ? t("status.conflicts", { n: status.conflicts }) : t("status.synced"),
      });

  return (
    <>
      <div className="ws-head">
        <div className="ws-title">
          <strong title={project.name}>{project.name}</strong>
          <span className="host-chip" title={`${project.server.user}@${project.server.host}`}>
            🖥 {project.server.user}@{project.server.host}
          </span>
        </div>
        <div className="ws-head-actions">
          <span className={offline ? "conn-pill" : "conn-pill ok"}>
            <StatusDot state={offline ? "offline" : status.state} />
            {connectionLabel}
          </span>
          <button
            type="button"
            className="btn btn-ghost btn-sm"
            onClick={() => void api.openLocalFolder(project.id)}
          >
            {t("workspace.openLocalFolder")}
          </button>
        </div>
      </div>

      <div className="ws-tabs" role="tablist">
        {tabs.map(([key, label]) => (
          <button
            key={key}
            type="button"
            role="tab"
            aria-selected={tab === key}
            className={`ws-tab ${tab === key ? "active" : ""}`}
            onClick={() => setTab(key)}
          >
            {label}
            {key === "conflicts" && conflicts.length > 0 && (
              <span className="badge">{conflicts.length}</span>
            )}
          </button>
        ))}
      </div>

      {error && <div className="banner error">{error}</div>}

      {tab === "terminal" &&
        (offline ? (
          <div className="term-wrap">
            <div className="term-overlay">{t("offline.banner")}</div>
          </div>
        ) : (
          <TerminalPane
            key={project.id}
            projectId={project.id}
            host={`${project.server.user}@${project.server.host}`}
          />
        ))}

      {tab === "files" && (
        <FilesExplorer
          key={project.id}
          projectId={project.id}
          projectName={project.name}
          conflictPaths={conflicts.map((conflict) => conflict.path)}
          onGoToConflicts={() => setTab("conflicts")}
        />
      )}

      {tab === "conflicts" && (
        <ConflictsView
          projectId={project.id}
          conflicts={conflicts}
          onChanged={async () => {
            await loadConflicts();
            await onStatusChanged();
          }}
        />
      )}
    </>
  );
}
