import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

const CANONICAL_UUID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const NIL_UUID = "00000000-0000-0000-0000-000000000000";
const CLEANUP_FAILURE_EVENT = "fns-onboarding-cleanup-failed";
let pendingUnmountCleanupFailure = "";

function isCanonicalWorkspaceId(value: string) {
  return CANONICAL_UUID.test(value) && value !== NIL_UUID;
}

function stableFailure(error: unknown) {
  if (typeof error === "string") {
    return error;
  }
  if (error && typeof error === "object" && "primary" in error) {
    const primary = Reflect.get(error, "primary");
    if (typeof primary === "string") {
      const cleanup = Reflect.get(error, "cleanup");
      if (Array.isArray(cleanup)) {
        const codes = cleanup.filter(
          (code): code is string => typeof code === "string",
        );
        if (codes.length > 0) {
          return `${primary}; cleanup=${codes.join(",")}`;
        }
      }
      return primary;
    }
  }
  return "operation_failed";
}

function rememberUnmountCleanupFailure(failures: string[]) {
  if (failures.length === 0) return;
  pendingUnmountCleanupFailure = `Cleanup failed; ${failures.join("; ")}`;
  window.dispatchEvent(
    new CustomEvent(CLEANUP_FAILURE_EVENT, {
      detail: pendingUnmountCleanupFailure,
    }),
  );
}

function takeUnmountCleanupFailure() {
  const failure = pendingUnmountCleanupFailure;
  pendingUnmountCleanupFailure = "";
  return failure;
}

interface SshHost {
  alias: string;
  hostname: string | null;
  port: number | null;
  user: string | null;
}

interface CredentialCleanupStatus {
  active: boolean;
  pendingAgentDeletion: boolean;
  pendingRevocation: boolean;
  pendingTunnelCleanup: boolean;
  lastError: string | null;
}

interface CredentialRollbackStatus extends CredentialCleanupStatus {
  credentialDeleted: boolean;
}

type DeployStep =
  | "validate_remote"
  | "ensure_directories"
  | "upload_server"
  | "upload_agent"
  | "verify_artifacts"
  | "prepare_configuration"
  | "switch_version"
  | "install_services"
  | "start_services"
  | "verify_health";

interface DeploymentPreview {
  previewId: string;
  target: string;
  version: string;
  serviceManager: "system" | "user" | null;
  existingVersion: string | null;
  artifacts: Array<{
    kind: string;
    sha256: string;
    bytes: number;
  }>;
  steps: DeployStep[];
  warnings: string[];
}

interface DeployProgress {
  projectId: string;
  step: DeployStep;
  status: "running" | "succeeded" | "failed";
  errorCode: string | null;
}

const DEPLOY_STEP_LABELS: Record<DeployStep, string> = {
  validate_remote: "Validate remote host",
  ensure_directories: "Prepare workspace directories",
  upload_server: "Upload server",
  upload_agent: "Upload remote sync agent",
  verify_artifacts: "Verify uploaded files",
  prepare_configuration: "Prepare persistent configuration",
  switch_version: "Switch active version",
  install_services: "Install managed services",
  start_services: "Start services",
  verify_health: "Verify server and sync agent",
};

const DEPLOY_WARNING_LABELS: Record<string, string> = {
  systemd_unavailable: "Managed services are not available on this host.",
  server_config_missing: "The existing server configuration could not be found.",
  insufficient_disk: "The remote host does not have enough free disk space.",
};

function pendingCredentialCleanup(status: CredentialCleanupStatus) {
  if (status.pendingAgentDeletion) {
    return `Agent credential deletion pending (${status.lastError ?? "credential_deletion_pending"})`;
  }
  if (status.pendingRevocation || status.pendingTunnelCleanup) {
    return `credential cleanup pending (${status.lastError ?? "cleanup_pending"})`;
  }
  if (status.active) {
    return "credential cleanup still active";
  }
  return "";
}

export default function OnboardingWizard({
  onComplete,
  onCancel,
}: {
  onComplete: () => void;
  onCancel: () => void;
}) {
  const [step, setStep] = useState(0);
  const [sshHosts, setSshHosts] = useState<SshHost[]>([]);
  const [error, setError] = useState(takeUnmountCleanupFailure);
  const [saving, setSaving] = useState(false);
  const operationGeneration = useRef(0);
  const mounted = useRef(true);
  const provisioningStarted = useRef(false);
  const deploymentStarted = useRef(false);
  const cancelling = useRef(false);
  const inFlight = useRef<Promise<unknown> | null>(null);
  const progressUnlisten = useRef<UnlistenFn | null>(null);

  const [projectName, setProjectName] = useState("");
  const [projectId] = useState(() => crypto.randomUUID());
  const [sshAlias, setSshAlias] = useState("");
  const [workspaceId, setWorkspaceId] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [remoteRoot, setRemoteRoot] = useState("");
  const [localRoot, setLocalRoot] = useState("");
  const [includes, setIncludes] = useState("**");
  const [excludes, setExcludes] = useState("");
  const [deploymentPreview, setDeploymentPreview] =
    useState<DeploymentPreview | null>(null);
  const [deploymentProgress, setDeploymentProgress] = useState<
    DeployProgress[]
  >([]);

  useEffect(() => {
    invoke<SshHost[]>("parse_ssh_hosts")
      .then(setSshHosts)
      .catch(() => {});
  }, []);

  useEffect(() => {
    const showCleanupFailure = (event: Event) => {
      if (event instanceof CustomEvent && typeof event.detail === "string") {
        setError(event.detail);
      }
    };
    window.addEventListener(CLEANUP_FAILURE_EVENT, showCleanupFailure);
    return () => {
      window.removeEventListener(CLEANUP_FAILURE_EVENT, showCleanupFailure);
    };
  }, []);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      operationGeneration.current += 1;
      progressUnlisten.current?.();
      progressUnlisten.current = null;
      if (provisioningStarted.current) {
        void cleanupProvisioning().then(rememberUnmountCleanupFailure);
      }
    };
  }, [projectId]);

  const steps = [
    "SSH Host",
    "Project Paths",
    "Sync Rules",
    "Review & Deploy",
  ];

  function next() {
    if (saving) return;
    setError("");
    if (step === 0) {
      if (!sshAlias) {
        setError("Please select or enter an SSH host.");
        return;
      }
      if (!isCanonicalWorkspaceId(workspaceId.trim())) {
        setError("Enter the server-configured workspace UUID.");
        return;
      }
      if (!username || !password) {
        setError("Enter your server username and password.");
        return;
      }
    }
    if (step === 1 && (!projectName || !remoteRoot || !localRoot)) {
      setError("Please fill in all fields.");
      return;
    }
    if (step === steps.length - 2) {
      setStep(steps.length - 1);
      void previewDeployment();
      return;
    }
    setStep((s) => Math.min(s + 1, steps.length - 1));
  }

  function back() {
    if (saving) return;
    setError("");
    setDeploymentPreview(null);
    setDeploymentProgress([]);
    setStep((s) => Math.max(s - 1, 0));
  }

  function deploymentRequest() {
    return {
      projectId,
      sshHostAlias: sshAlias,
      workspaceId: workspaceId.trim(),
      remoteRoot,
      includes: includes.split("\n").filter(Boolean),
      excludes: excludes.split("\n").filter(Boolean),
      protectSecrets: true,
    };
  }

  async function trackInvoke<T>(operation: Promise<T>) {
    inFlight.current = operation;
    try {
      return await operation;
    } finally {
      if (inFlight.current === operation) {
        inFlight.current = null;
      }
    }
  }

  async function cleanupProvisioning() {
    const failures: string[] = [];
    if (deploymentStarted.current) {
      try {
        await invoke("cancel_remote_deployment", { projectId });
      } catch (failure) {
        failures.push(`deployment cancel failed (${stableFailure(failure)})`);
      }
      deploymentStarted.current = false;
    }
    let credentialDeleted = false;
    try {
      const status = await invoke<CredentialRollbackStatus>(
        "cancel_workspace_provisioning",
        { projectId },
      );
      credentialDeleted = status.credentialDeleted;
      if (!credentialDeleted) {
        failures.push("credential cleanup did not delete the Agent credential");
      }
      const pending = pendingCredentialCleanup(status);
      if (pending) failures.push(pending);
    } catch (failure) {
      failures.push(`cancel failed (${stableFailure(failure)})`);
      try {
        const status = await invoke<CredentialCleanupStatus>(
          "workspace_credential_cleanup_status",
          { projectId },
        );
        const pending = pendingCredentialCleanup(status);
        if (pending) failures.push(pending);
      } catch (statusFailure) {
        failures.push(`cleanup status failed (${stableFailure(statusFailure)})`);
      }
    }
    try {
      await inFlight.current;
    } catch (failure) {
      const code = stableFailure(failure);
      if (code !== "cancelled" && code !== "timeout") {
        failures.push(`provisioning failed (${code})`);
      }
    }
    if (provisioningStarted.current && credentialDeleted) {
      let projectDeleted = false;
      try {
        await invoke("delete_project", { id: projectId });
        projectDeleted = true;
      } catch (failure) {
        failures.push(`project cleanup failed (${stableFailure(failure)})`);
      }
      if (projectDeleted) {
        provisioningStarted.current = false;
      }
    }
    return failures;
  }

  async function previewDeployment() {
    if (saving) return;
    const generation = operationGeneration.current + 1;
    operationGeneration.current = generation;
    setSaving(true);
    setError("");
    setDeploymentPreview(null);
    setDeploymentProgress([]);
    try {
      const preview = await trackInvoke(
        invoke<DeploymentPreview>("preview_remote_deployment", {
          request: deploymentRequest(),
        }),
      );
      if (generation !== operationGeneration.current || !mounted.current) return;
      setDeploymentPreview(preview);
    } catch (failure) {
      if (generation !== operationGeneration.current || !mounted.current) return;
      setError(`Remote check failed (${stableFailure(failure)})`);
    } finally {
      if (generation === operationGeneration.current && mounted.current) {
        setSaving(false);
      }
    }
  }

  async function cancelWizard() {
    if (cancelling.current) return;
    cancelling.current = true;
    const generation = operationGeneration.current + 1;
    operationGeneration.current = generation;
    setSaving(true);
    setError("");
    const cleanupFailures = await cleanupProvisioning();
    if (!mounted.current || generation !== operationGeneration.current) return;
    setPassword("");
    setSaving(false);
    if (cleanupFailures.length > 0) {
      setError(`Cancel failed; ${cleanupFailures.join("; ")}`);
      cancelling.current = false;
      return;
    }
    cancelling.current = false;
    onCancel();
  }

  async function deploy() {
    if (saving || !deploymentPreview || deploymentPreview.warnings.length > 0)
      return;
    const generation = operationGeneration.current + 1;
    operationGeneration.current = generation;
    setSaving(true);
    setError("");
    provisioningStarted.current = true;
    try {
      await trackInvoke(
        invoke("provision_workspace_credential", {
          request: {
            projectId,
            sshHostAlias: sshAlias,
            username,
            password,
          },
        }),
      );
      if (generation !== operationGeneration.current || !mounted.current) return;

      await trackInvoke(
        invoke("save_project", {
          config: {
            id: projectId,
            name: projectName,
            sshHostAlias: sshAlias,
            remoteRoot: remoteRoot,
            localRoot: localRoot,
            workspaceId: workspaceId.trim(),
            tmuxSession: `fns-${projectName}`,
            sync: {
              mode: "two_way_safe",
              includes: includes.split("\n").filter(Boolean),
              excludes: excludes.split("\n").filter(Boolean),
              protectSecrets: true,
            },
          },
        }),
      );
      if (generation !== operationGeneration.current || !mounted.current) return;

      progressUnlisten.current?.();
      progressUnlisten.current = await listen<DeployProgress>(
        "deploy://progress",
        ({ payload }) => {
          if (payload.projectId !== projectId || !mounted.current) return;
          setDeploymentProgress((current) => {
            const withoutStep = current.filter(
              (entry) => entry.step !== payload.step,
            );
            return [...withoutStep, payload];
          });
        },
      );
      deploymentStarted.current = true;
      await trackInvoke(
        invoke("execute_remote_deployment", {
          previewId: deploymentPreview.previewId,
          request: deploymentRequest(),
        }),
      );
      deploymentStarted.current = false;
      progressUnlisten.current?.();
      progressUnlisten.current = null;
      if (generation !== operationGeneration.current || !mounted.current) return;

      // Probe after deployment: the workspace root is registered and the
      // server restarted during execute_remote_deployment, so the workspace
      // is now accessible. Probing before deployment always failed for new
      // workspaces because the root had not been registered yet.
      await trackInvoke(
        invoke("probe_workspace_access", {
          request: {
            projectId,
            sshHostAlias: sshAlias,
            workspaceId: workspaceId.trim(),
          },
        }),
      );
      if (generation !== operationGeneration.current || !mounted.current) return;

      provisioningStarted.current = false;
      onComplete();
    } catch (failure) {
      if (generation !== operationGeneration.current || !mounted.current) return;
      const cleanupFailures = await cleanupProvisioning();
      if (generation !== operationGeneration.current || !mounted.current) return;
      const cleanupFailure = cleanupFailures.length
        ? `; ${cleanupFailures.join("; ")}`
        : "";
      setError(`Setup failed (${stableFailure(failure)})${cleanupFailure}`);
      setDeploymentPreview(null);
    } finally {
      progressUnlisten.current?.();
      progressUnlisten.current = null;
      if (generation === operationGeneration.current && mounted.current) {
        setPassword("");
        setSaving(false);
      }
    }
  }

  return (
    <div className="wizard">
      <h1>Set Up Remote Workspace</h1>
      <div style={{ display: "flex", gap: "4px", marginBottom: "24px" }}>
        {steps.map((s, i) => (
          <div
            key={s}
            style={{
              flex: 1,
              padding: "8px",
              textAlign: "center",
              fontSize: "12px",
              borderRadius: "4px",
              background: i <= step ? "#2563eb" : "#e0e0e0",
              color: i <= step ? "white" : "#666",
            }}
          >
            {i + 1}. {s}
          </div>
        ))}
      </div>

      {step === 0 && (
        <>
          <div className="wizard-step">
            <label>SSH Host</label>
            {sshHosts.length > 0 ? (
              <select
                value={sshAlias}
                onChange={(e) => setSshAlias(e.target.value)}
              >
                <option value="">— Select a host —</option>
                {sshHosts.map((h) => (
                  <option key={h.alias} value={h.alias}>
                    {h.alias}
                    {h.hostname ? ` (${h.hostname})` : ""}
                  </option>
                ))}
              </select>
            ) : (
              <input
                type="text"
                placeholder="user@hostname or alias"
                value={sshAlias}
                onChange={(e) => setSshAlias(e.target.value)}
              />
            )}
          </div>
          <div className="wizard-step">
            <label>Workspace UUID</label>
            <input
              type="text"
              spellCheck={false}
              autoCapitalize="none"
              placeholder="server-configured workspace UUID"
              value={workspaceId}
              onChange={(e) => setWorkspaceId(e.target.value)}
            />
          </div>
          <div className="wizard-step">
            <label>Server Username or Email</label>
            <input
              type="text"
              autoComplete="username"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
            />
          </div>
          <div className="wizard-step">
            <label>Server Password</label>
            <input
              type="password"
              autoComplete="current-password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
          </div>
        </>
      )}

      {step === 1 && (
        <>
          <div className="wizard-step">
            <label>Project Name</label>
            <input
              type="text"
              value={projectName}
              onChange={(e) => setProjectName(e.target.value)}
              placeholder="my-project"
            />
          </div>
          <div className="wizard-step">
            <label>Remote Project Root</label>
            <input
              type="text"
              value={remoteRoot}
              onChange={(e) => setRemoteRoot(e.target.value)}
              placeholder="/home/user/projects/my-project"
            />
          </div>
          <div className="wizard-step">
            <label>Local Sync Directory</label>
            <input
              type="text"
              value={localRoot}
              onChange={(e) => setLocalRoot(e.target.value)}
              placeholder="/Users/user/Projects/my-project"
            />
          </div>
        </>
      )}

      {step === 2 && (
        <>
          <div className="wizard-step">
            <label>Include Patterns (one per line)</label>
            <textarea
              value={includes}
              onChange={(e) => setIncludes(e.target.value)}
              rows={3}
              style={{ width: "100%", padding: "8px" }}
            />
          </div>
          <div className="wizard-step">
            <label>Exclude Patterns (one per line)</label>
            <textarea
              value={excludes}
              onChange={(e) => setExcludes(e.target.value)}
              rows={5}
              style={{ width: "100%", padding: "8px" }}
              placeholder=".git/&#10;node_modules/&#10;target/"
            />
          </div>
        </>
      )}

      {step === 3 && (
        <>
          <div className="wizard-step">
            <strong>Project:</strong> {projectName}
          </div>
          <div className="wizard-step">
            <strong>SSH Host:</strong> {sshAlias}
          </div>
          <div className="wizard-step">
            <strong>Workspace:</strong> {workspaceId.trim()}
          </div>
          <div className="wizard-step">
            <strong>Remote:</strong> {remoteRoot}
          </div>
          <div className="wizard-step">
            <strong>Local:</strong> {localRoot}
          </div>
          <p style={{ color: "#666", marginTop: "16px" }}>
            {saving && !deploymentPreview
              ? "Checking the remote host..."
              : deploymentPreview
                ? `Ready to install ${deploymentPreview.version} on ${deploymentPreview.target}.`
                : "The remote host has not been checked yet."}
          </p>
          {deploymentPreview && (
            <div className="deployment-review">
              <div className="wizard-step">
                <strong>Service manager:</strong>{" "}
                {deploymentPreview.serviceManager ?? "Unavailable"}
              </div>
              {deploymentPreview.existingVersion && (
                <div className="wizard-step">
                  <strong>Current version:</strong>{" "}
                  {deploymentPreview.existingVersion}
                </div>
              )}
              <ol className="deployment-steps">
                {deploymentPreview.steps.map((deployStep) => {
                  const progress = deploymentProgress.find(
                    (entry) => entry.step === deployStep,
                  );
                  return (
                    <li key={deployStep} data-status={progress?.status ?? "pending"}>
                      <span>{DEPLOY_STEP_LABELS[deployStep]}</span>
                      <span>{progress?.status ?? "pending"}</span>
                    </li>
                  );
                })}
              </ol>
              {deploymentPreview.warnings.map((warning) => (
                <p className="error" key={warning}>
                  {DEPLOY_WARNING_LABELS[warning] ?? warning}
                </p>
              ))}
            </div>
          )}
        </>
      )}

      {error && <p className="error">{error}</p>}

      <div className="wizard-nav">
        {step > 0 ? (
          <button
            className="btn btn-secondary"
            onClick={back}
            disabled={saving}
          >
            Back
          </button>
        ) : (
          <button className="btn btn-secondary" onClick={cancelWizard}>
            Cancel
          </button>
        )}
        {step < steps.length - 1 ? (
          <button
            className="btn btn-primary"
            onClick={next}
            disabled={saving}
          >
            Next
          </button>
        ) : (
          <>
            {(!deploymentPreview || deploymentPreview.warnings.length > 0) &&
              !saving && (
              <button className="btn btn-secondary" onClick={previewDeployment}>
                Check again
              </button>
              )}
            <button
              className="btn btn-primary"
              onClick={deploy}
              disabled={
                saving ||
                !deploymentPreview ||
                deploymentPreview.warnings.length > 0
              }
            >
              {saving
                ? deploymentStarted.current
                  ? "Deploying..."
                  : "Checking..."
                : "Deploy"}
            </button>
          </>
        )}
      </div>
    </div>
  );
}
