import { type FormEvent, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import TerminalPane from "./Terminal";
import FileTree from "./FileTree";
import ConflictsPane from "./ConflictsPane";
import { DiagnosticsPane } from "../features/diagnostics";
import {
  ClaudeSessionsPane,
  ServerStatusPane,
} from "../features/remote-monitor";
import { createInvokeDiagnosticsClient } from "../lib/diagnosticsApi";
import { createRemoteMonitorClient } from "../lib/remoteMonitorApi";
import {
  accountConnectionFailureMessage,
  isAuthenticationFailure,
} from "../auth";

interface Project {
  id: string;
  name: string;
  sshHostAlias: string;
  remoteRoot: string;
  localRoot: string;
  workspaceId: string;
  tmuxSession: string;
}

interface SyncFailure {
  primary: string;
  cleanup: string[];
}

interface SyncStatus {
  running: boolean;
  localPort: number | null;
  message: string;
  error: SyncFailure | null;
}

interface WorkspaceViewProps {
  project: Project;
  startupFailure?: unknown;
  credentialRequired?: boolean;
  onRetryStart: () => Promise<unknown | null>;
}

type Tab =
  | "terminal"
  | "files"
  | "conflicts"
  | "logs"
  | "server-status"
  | "claude-sessions";
type Action = "start" | "stop";

const SYNC_POLL_INTERVAL_MS = 2000;
function structuredFailure(error: unknown): SyncFailure | null {
  if (!error || typeof error !== "object" || !("primary" in error)) {
    return null;
  }
  const cleanup = "cleanup" in error && Array.isArray(error.cleanup)
    ? error.cleanup.map(String)
    : [];
  return { primary: String(error.primary), cleanup };
}

function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  try {
    return JSON.stringify(error) ?? "Unknown error";
  } catch {
    return "Unknown error";
  }
}

function FailureDetails({
  label,
  error,
}: {
  label: string;
  error: unknown;
}) {
  const failure = structuredFailure(error);
  return (
    <div className="sync-error" role="alert">
      <strong>{label}</strong>
      {failure ? (
        <dl>
          <div>
            <dt>Primary</dt>
            <dd>{failure.primary}</dd>
          </div>
          <div>
            <dt>Cleanup</dt>
            <dd>{failure.cleanup.length > 0 ? failure.cleanup.join(", ") : "None"}</dd>
          </div>
        </dl>
      ) : (
        <span>{errorMessage(error)}</span>
      )}
    </div>
  );
}

export default function WorkspaceView({
  project,
  startupFailure,
  credentialRequired = false,
  onRetryStart,
}: WorkspaceViewProps) {
  const [activeTab, setActiveTab] = useState<Tab>("terminal");
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [statusFailure, setStatusFailure] = useState<unknown>(null);
  const [actionFailure, setActionFailure] = useState<{
    action: Action;
    error: unknown;
  } | null>(null);
  const [activeAction, setActiveAction] = useState<Action | null>(null);
  const [lastRefresh, setLastRefresh] = useState<Date | null>(null);
  const [refreshVersion, setRefreshVersion] = useState(0);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [connecting, setConnecting] = useState(false);
  const [connectionFailure, setConnectionFailure] = useState<string | null>(
    null,
  );
  const [connectionNotice, setConnectionNotice] = useState<string | null>(null);
  const connectionGeneration = useRef(0);
  const connectionInFlight = useRef(false);
  const diagnosticsClient = useMemo(
    () => createInvokeDiagnosticsClient(invoke),
    [],
  );
  const remoteMonitorClient = useMemo(
    () => createRemoteMonitorClient(invoke),
    [],
  );

  const authenticationRequired = Boolean(
    credentialRequired ||
      isAuthenticationFailure(startupFailure) ||
      isAuthenticationFailure(statusFailure) ||
      isAuthenticationFailure(actionFailure?.error) ||
      isAuthenticationFailure(status?.message) ||
      isAuthenticationFailure(status?.error),
  );

  useEffect(() => {
    connectionGeneration.current += 1;
    connectionInFlight.current = false;
    setUsername("");
    setPassword("");
    setConnecting(false);
    setConnectionFailure(null);
    setConnectionNotice(null);

    return () => {
      connectionGeneration.current += 1;
      connectionInFlight.current = false;
    };
  }, [project.id]);

  useEffect(() => {
    if (!authenticationRequired) {
      setConnectionFailure(null);
      setConnectionNotice(null);
    }
  }, [authenticationRequired]);

  useEffect(() => {
    let cancelled = false;
    let pollTimer: ReturnType<typeof setTimeout> | undefined;

    setStatus(null);
    setStatusFailure(null);
    setLastRefresh(null);

    async function pollStatus() {
      try {
        const next = await invoke<SyncStatus>("sync_status", {
          projectId: project.id,
        });
        if (cancelled) return;
        setStatus(next);
        setStatusFailure(null);
        setLastRefresh(new Date());
      } catch (error) {
        if (cancelled) return;
        setStatusFailure(error);
      } finally {
        if (!cancelled) {
          pollTimer = window.setTimeout(pollStatus, SYNC_POLL_INTERVAL_MS);
        }
      }
    }

    void pollStatus();
    return () => {
      cancelled = true;
      if (pollTimer !== undefined) clearTimeout(pollTimer);
    };
  }, [project.id, refreshVersion]);

  async function retryStart() {
    setActiveAction("start");
    setActionFailure(null);
    const error = await onRetryStart();
    if (error) setActionFailure({ action: "start", error });
    setActiveAction(null);
    setRefreshVersion((version) => version + 1);
  }

  async function stopSync() {
    setActiveAction("stop");
    setActionFailure(null);
    try {
      await invoke("stop_sync", { projectId: project.id });
    } catch (error) {
      setActionFailure({ action: "stop", error });
    } finally {
      setActiveAction(null);
      setRefreshVersion((version) => version + 1);
    }
  }

  async function reconnectAccount(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (connectionInFlight.current) return;

    const normalizedUsername = username.trim();
    if (!normalizedUsername || !password) {
      setConnectionFailure("Enter your username and password.");
      return;
    }

    const generation = connectionGeneration.current + 1;
    connectionGeneration.current = generation;
    connectionInFlight.current = true;
    setConnecting(true);
    setConnectionFailure(null);
    setConnectionNotice(null);
    setActionFailure(null);

    try {
      if (status?.running) {
        await invoke("stop_sync", { projectId: project.id });
        if (generation !== connectionGeneration.current) return;
      }

      await invoke("reprovision_workspace_credential", {
        request: {
          projectId: project.id,
          sshHostAlias: project.sshHostAlias,
          username: normalizedUsername,
          password,
        },
      });
      if (generation !== connectionGeneration.current) return;

      await invoke("probe_workspace_access", {
        request: {
          projectId: project.id,
          sshHostAlias: project.sshHostAlias,
          workspaceId: project.workspaceId,
        },
      });
      if (generation !== connectionGeneration.current) return;

      const startError = await onRetryStart();
      if (generation !== connectionGeneration.current) return;
      if (startError) {
        setConnectionFailure(accountConnectionFailureMessage(startError));
        return;
      }

      setConnectionNotice("Account connected. Starting sync...");
      setRefreshVersion((version) => version + 1);
    } catch (failure) {
      if (generation !== connectionGeneration.current) return;
      setConnectionFailure(accountConnectionFailureMessage(failure));
    } finally {
      if (generation === connectionGeneration.current) {
        setPassword("");
        setConnecting(false);
        connectionInFlight.current = false;
      }
    }
  }

  const tabs: { key: Tab; label: string }[] = [
    { key: "terminal", label: "Terminal" },
    { key: "files", label: "Files" },
    { key: "conflicts", label: "Conflicts" },
    { key: "logs", label: "Logs" },
    { key: "server-status", label: "Server Status" },
    { key: "claude-sessions", label: "Claude Sessions" },
  ];
  const hasFailure = Boolean(
    startupFailure || statusFailure || actionFailure || status?.error,
  );
  const statusTone = hasFailure
    ? "error"
    : status?.running
      ? "running"
      : "stopped";

  return (
    <main className="main-content workspace-view">
      <header className="workspace-header">
        <div className="workspace-title">
          <strong>{project.name}</strong>
          <span>{project.sshHostAlias}</span>
        </div>
        <div className="workspace-tabs" role="tablist" aria-label="Workspace views">
          {tabs.map((tab) => (
            <button
              key={tab.key}
              className={activeTab === tab.key ? "active" : ""}
              role="tab"
              aria-selected={activeTab === tab.key}
              onClick={() => setActiveTab(tab.key)}
            >
              {tab.label}
            </button>
          ))}
        </div>
      </header>

      {authenticationRequired ? (
        <section className="account-connection-panel" aria-live="polite">
          <div className="account-connection-heading">
            <span className="account-state-dot" aria-hidden="true" />
            <div>
              <strong>Connect your account</strong>
              <span>Sign in to resume file sync.</span>
            </div>
          </div>
          <form className="account-connection-form" onSubmit={reconnectAccount}>
            <label>
              <span>Username or email</span>
              <input
                type="text"
                autoComplete="username"
                value={username}
                onChange={(event) => setUsername(event.target.value)}
                disabled={connecting}
                required
              />
            </label>
            <label>
              <span>Password</span>
              <input
                type="password"
                autoComplete="current-password"
                value={password}
                onChange={(event) => setPassword(event.target.value)}
                disabled={connecting}
                required
              />
            </label>
            <button
              className="btn btn-primary"
              type="submit"
              disabled={connecting || !username.trim() || !password}
            >
              {connecting ? "Connecting..." : "Connect and resume"}
            </button>
          </form>
          {(connectionFailure || connectionNotice) && (
            <p
              className={
                connectionFailure
                  ? "account-connection-feedback account-connection-error"
                  : "account-connection-feedback account-connection-notice"
              }
              role={connectionFailure ? "alert" : "status"}
            >
              {connectionFailure ?? connectionNotice}
            </p>
          )}
        </section>
      ) : (
        <section className={`sync-panel sync-panel-${statusTone}`} aria-live="polite">
          <div className="sync-summary">
            <span className="sync-state-dot" aria-hidden="true" />
            <div>
              <strong>{status ? (status.running ? "Running" : "Stopped") : "Checking"}</strong>
              <span>Phase: {status?.message ?? "Waiting for status"}</span>
            </div>
          </div>
          <dl className="sync-metadata">
            <div>
              <dt>Local port</dt>
              <dd>{status?.localPort ?? "Not assigned"}</dd>
            </div>
            <div>
              <dt>Last refresh</dt>
              <dd>{lastRefresh ? lastRefresh.toLocaleTimeString() : "Not yet"}</dd>
            </div>
          </dl>
          <div className="sync-actions">
            <button
              className="btn btn-secondary"
              disabled={activeAction !== null}
              onClick={() => void retryStart()}
            >
              {activeAction === "start" ? "Starting..." : "Retry start"}
            </button>
            <button
              className="btn btn-danger"
              disabled={activeAction !== null || !status?.running}
              onClick={() => void stopSync()}
            >
              {activeAction === "stop" ? "Stopping..." : "Stop sync"}
            </button>
          </div>
        </section>
      )}

      {!authenticationRequired &&
        (startupFailure || statusFailure || actionFailure || status?.error) && (
        <div className="sync-errors">
          {startupFailure !== null && startupFailure !== undefined && (
            <FailureDetails label="Initial start failed" error={startupFailure} />
          )}
          {statusFailure !== null && statusFailure !== undefined && (
            <FailureDetails label="Status refresh failed" error={statusFailure} />
          )}
          {actionFailure && (
            <FailureDetails
              label={`${actionFailure.action === "start" ? "Start" : "Stop"} failed`}
              error={actionFailure.error}
            />
          )}
          {status?.error && (
            <FailureDetails label="Sync runtime error" error={status.error} />
          )}
        </div>
        )}

      <div className="workspace-body">
        {activeTab === "terminal" && (
          <TerminalPane
            projectId={project.id}
            sshHostAlias={project.sshHostAlias}
            remoteRoot={project.remoteRoot}
            tmuxSession={project.tmuxSession || `fns-${project.name}`}
          />
        )}
        {activeTab === "files" && <FileTree projectId={project.id} />}
        {activeTab === "conflicts" && (
          <ConflictsPane
            projectId={project.id}
            syncRunning={Boolean(status?.running)}
          />
        )}
        {activeTab === "logs" && (
          <DiagnosticsPane
            projectId={project.id}
            client={diagnosticsClient}
          />
        )}
        {activeTab === "server-status" && (
          <ServerStatusPane
            projectId={project.id}
            client={remoteMonitorClient}
          />
        )}
        {activeTab === "claude-sessions" && (
          <ClaudeSessionsPane
            projectId={project.id}
            client={remoteMonitorClient}
            onRequestTerminalTab={() => setActiveTab("terminal")}
          />
        )}
      </div>
    </main>
  );
}
