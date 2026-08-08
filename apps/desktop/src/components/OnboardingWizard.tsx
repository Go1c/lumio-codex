import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

interface SshHost {
  alias: string;
  hostname: string | null;
  port: number | null;
  user: string | null;
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
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);

  const [projectName, setProjectName] = useState("");
  const [sshAlias, setSshAlias] = useState("");
  const [remoteRoot, setRemoteRoot] = useState("");
  const [localRoot, setLocalRoot] = useState("");
  const [includes, setIncludes] = useState("**");
  const [excludes, setExcludes] = useState("");

  useEffect(() => {
    invoke<SshHost[]>("parse_ssh_hosts")
      .then(setSshHosts)
      .catch(() => {});
  }, []);

  const steps = [
    "SSH Host",
    "Project Paths",
    "Sync Rules",
    "Review & Deploy",
  ];

  function next() {
    setError("");
    if (step === 0 && !sshAlias) {
      setError("Please select or enter an SSH host.");
      return;
    }
    if (step === 1 && (!projectName || !remoteRoot || !localRoot)) {
      setError("Please fill in all fields.");
      return;
    }
    setStep((s) => Math.min(s + 1, steps.length - 1));
  }

  function back() {
    setError("");
    setStep((s) => Math.max(s - 1, 0));
  }

  async function deploy() {
    setSaving(true);
    setError("");
    try {
      // Save project configuration.
      await invoke("save_project", {
        config: {
          id: crypto.randomUUID(),
          name: projectName,
          sshHostAlias: sshAlias,
          remoteRoot: remoteRoot,
          localRoot: localRoot,
          workspaceId: crypto.randomUUID(),
          tmuxSession: `fns-${projectName}`,
          sync: {
            mode: "two_way_safe",
            includes: includes.split("\n").filter(Boolean),
            excludes: excludes.split("\n").filter(Boolean),
            protectSecrets: true,
          },
        },
      });

      // Create SSH tunnel to the server's FNS Server.
      try {
        const localPort = await invoke<number>("create_tunnel", {
          sshAlias,
          remotePort: 9000,
        });
        console.log(`SSH tunnel created on local port ${localPort}`);
      } catch (tunnelErr) {
        console.warn("SSH tunnel creation failed:", tunnelErr);
        // Non-fatal — user can connect manually.
      }

      onComplete();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
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
            <strong>Remote:</strong> {remoteRoot}
          </div>
          <div className="wizard-step">
            <strong>Local:</strong> {localRoot}
          </div>
          <p style={{ color: "#666", marginTop: "16px" }}>
            Click "Deploy" to upload and start the FNS Server and agent on the
            remote host, then begin initial sync.
          </p>
        </>
      )}

      {error && <p className="error">{error}</p>}

      <div className="wizard-nav">
        {step > 0 ? (
          <button className="btn btn-secondary" onClick={back}>
            Back
          </button>
        ) : (
          <button className="btn btn-secondary" onClick={onCancel}>
            Cancel
          </button>
        )}
        {step < steps.length - 1 ? (
          <button className="btn btn-primary" onClick={next}>
            Next
          </button>
        ) : (
          <button
            className="btn btn-primary"
            onClick={deploy}
            disabled={saving}
          >
            {saving ? "Deploying..." : "Deploy"}
          </button>
        )}
      </div>
    </div>
  );
}
