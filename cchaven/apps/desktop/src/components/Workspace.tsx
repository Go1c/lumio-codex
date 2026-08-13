import { useCallback, useEffect, useRef, useState } from "react";
import { t } from "../i18n";
import { toApiError } from "../lib/api";
import { useApi } from "../state/ApiProvider";
import { ConflictsView } from "./ConflictsView";
import { FilesExplorer } from "./FilesExplorer";
import { TerminalPane } from "./TerminalPane";
import { StatusDot } from "./ui";
import { ClaudeSessionsPane, ServerStatusPane } from "../features/remote-monitor";
import { DiagnosticsPane } from "../features/diagnostics";
import type {
  Conflict,
  ProjectConfig,
  SyncEngineStatus,
  SyncStatus,
} from "../lib/types";

export type WorkspaceTab =
  | "terminal"
  | "files"
  | "conflicts"
  | "claude-sessions"
  | "server-status"
  | "logs";

/** How often the workspace re-reads the engine status while it is on screen. */
const SYNC_POLL_INTERVAL_MS = 5_000;

/** 5.5 工作区：顶部信息栏 + 同步控制条 + 六个 Tab。 */
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
  const [activeTab, setActiveTab] = useState<WorkspaceTab>("terminal");
  const [conflicts, setConflicts] = useState<Conflict[]>([]);
  const [error, setError] = useState("");

  const [engine, setEngine] = useState<SyncEngineStatus | null>(null);
  const [lastRefresh, setLastRefresh] = useState<Date | null>(null);
  const [syncBusy, setSyncBusy] = useState(false);
  const [syncError, setSyncError] = useState("");

  // The panes are dependency-injected so browser mock mode drives the same
  // components the Tauri build does.
  const diagnosticsClient = api.diagnostics;
  const remoteMonitorClient = api.remoteMonitor;

  const loadConflicts = useCallback(async () => {
    try {
      setConflicts(await api.listConflicts(project.id));
    } catch (caught) {
      setError(toApiError(caught).message);
    }
  }, [api, project.id]);

  useEffect(() => {
    setActiveTab("terminal");
    void loadConflicts();
  }, [loadConflicts]);

  // Poll the engine on a bounded delay rather than an interval: a slow reply
  // must not queue a backlog of requests behind it.
  const pollTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    let cancelled = false;

    const tick = async () => {
      try {
        const next = await api.syncEngineStatus(project.id);
        if (cancelled) return;
        setEngine(next);
        setLastRefresh(new Date());
      } catch {
        if (cancelled) return;
        setEngine(null);
      }
      if (cancelled) return;
      pollTimer.current = setTimeout(() => void tick(), SYNC_POLL_INTERVAL_MS);
    };

    void tick();
    return () => {
      cancelled = true;
      if (pollTimer.current) clearTimeout(pollTimer.current);
      pollTimer.current = null;
    };
  }, [api, project.id]);

  async function runSyncAction(action: "start" | "stop") {
    setSyncBusy(true);
    setSyncError("");
    try {
      if (action === "start") setEngine(await api.startSync(project.id));
      else {
        await api.stopSync(project.id);
        setEngine(await api.syncEngineStatus(project.id));
      }
      setLastRefresh(new Date());
      await onStatusChanged();
    } catch (caught) {
      setSyncError(toApiError(caught).message);
    } finally {
      setSyncBusy(false);
    }
  }

  const tabs: Array<{ key: WorkspaceTab; label: string }> = [
    { key: "terminal", label: t("workspace.tabTerminal") },
    { key: "files", label: t("workspace.tabFiles") },
    { key: "conflicts", label: t("workspace.tabConflicts") },
    { key: "claude-sessions", label: t("sessions.title") },
    { key: "server-status", label: t("server.title") },
    { key: "logs", label: t("logs.title") },
  ];

  const connectionLabel = offline
    ? t("workspace.disconnected")
    : t("workspace.connected", {
        sync:
          status.state === "conflicts"
            ? t("status.conflicts", { n: status.conflicts })
            : t("status.synced"),
      });

  const failure = engine?.error ?? null;

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

      <div className="sync-bar">
        <span className="sync-bar-state">
          {engine?.running
            ? t("sync.engineRunning", { port: engine.localPort ?? 0 })
            : t("sync.engineStopped")}
        </span>
        {engine?.message && <span className="sync-bar-message">{engine.message}</span>}
        {failure && (
          <span className="sync-bar-failure">
            {t("sync.failurePrimary", { code: failure.primary })}
            {failure.cleanup.length > 0 &&
              ` ${t("sync.failureCleanup", { codes: failure.cleanup.join("、") })}`}
          </span>
        )}
        {lastRefresh && (
          <span className="sync-bar-refresh">
            {t("sync.lastRefresh", { time: lastRefresh.toLocaleTimeString() })}
          </span>
        )}
        <span className="sync-bar-actions">
          <button
            type="button"
            className="btn btn-secondary btn-sm"
            onClick={() => void runSyncAction("start")}
            disabled={syncBusy}
          >
            {engine?.running ? t("sync.retry") : t("sync.start")}
          </button>
          <button
            type="button"
            className="btn btn-ghost btn-sm"
            onClick={() => void runSyncAction("stop")}
            disabled={syncBusy || !engine?.running}
          >
            {t("sync.stop")}
          </button>
        </span>
      </div>
      {syncError && <div className="banner error">{syncError}</div>}

      <div className="ws-tabs" role="tablist">
        {tabs.map(({ key, label }) => (
          <button
            key={key}
            type="button"
            role="tab"
            aria-selected={activeTab === key}
            className={`ws-tab ${activeTab === key ? "active" : ""}`}
            onClick={() => setActiveTab(key)}
          >
            {label}
            {key === "conflicts" && conflicts.length > 0 && (
              <span className="badge">{conflicts.length}</span>
            )}
          </button>
        ))}
      </div>

      {error && <div className="banner error">{error}</div>}

      {activeTab === "terminal" &&
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

      {activeTab === "files" && (
        <FilesExplorer
          key={project.id}
          projectId={project.id}
          projectName={project.name}
          conflictPaths={conflicts.map((conflict) => conflict.path)}
          onGoToConflicts={() => setActiveTab("conflicts")}
        />
      )}

      {activeTab === "conflicts" && (
        <ConflictsView
          projectId={project.id}
          conflicts={conflicts}
          onChanged={async () => {
            await loadConflicts();
            await onStatusChanged();
          }}
        />
      )}

      {activeTab === "claude-sessions" && (
        <ClaudeSessionsPane
          projectId={project.id}
          client={remoteMonitorClient}
          onRequestTerminalTab={() => setActiveTab("terminal")}
        />
      )}

      {activeTab === "server-status" && (
        <ServerStatusPane projectId={project.id} client={remoteMonitorClient} />
      )}

      {activeTab === "logs" && (
        <DiagnosticsPane projectId={project.id} client={diagnosticsClient} />
      )}
    </>
  );
}
