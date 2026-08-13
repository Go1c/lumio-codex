import { useState } from "react";
import { t } from "../../i18n";
import type { DiagnosticsClient } from "../../lib/diagnosticsApi";

export default function SelfTestView({
  client,
}: {
  client: DiagnosticsClient;
}) {
  const [profile, setProfile] = useState("ci-isolation");
  const [runId, setRunId] = useState<string | null>(null);
  const [status, setStatus] = useState(t("logs.selfTestIdle"));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function onRun() {
    setBusy(true);
    setError(null);
    try {
      const result = await client.runSelfTest(profile.trim());
      setRunId(result.runId);
      const lines = [
        `Self test finished.`,
        `runId: ${result.runId}`,
        `profile: ${profile.trim()}`,
      ];
      if (result.outcome) lines.push(`outcome: ${result.outcome}`);
      if (result.manifestPath) lines.push(`manifest: ${result.manifestPath}`);
      if (result.bugPackagePath) lines.push(`bugPackage: ${result.bugPackagePath}`);
      setStatus(lines.join("\n"));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function onCancel() {
    if (!runId) return;
    setBusy(true);
    setError(null);
    try {
      await client.cancelSelfTest(runId);
      setStatus(`Self test cancelled.\nrunId: ${runId}`);
      setRunId(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="diagnostics-self-test" aria-label={t("logs.tabSelfTest")}>
      <div className="diagnostics-toolbar">
        <div className="diagnostics-field" style={{ minWidth: 220, flex: 1 }}>
          <label htmlFor="diagnostics-self-test-profile">{t("logs.profile")}</label>
          <input
            id="diagnostics-self-test-profile"
            value={profile}
            onChange={(e) => setProfile(e.target.value)}
            placeholder="test-only profile name"
            disabled={busy}
          />
        </div>
      </div>

      <div className="diagnostics-actions">
        <button
          type="button"
          className="btn btn-primary"
          onClick={onRun}
          disabled={busy || !profile.trim()}
        >
          Run self test
        </button>
        <button
          type="button"
          className="btn btn-danger"
          onClick={onCancel}
          disabled={busy || !runId}
        >
          Cancel
        </button>
      </div>

      {error && (
        <div className="diagnostics-error" role="alert">
          {error}
        </div>
      )}

      <div className="diagnostics-status" role="status">
        {status}
      </div>
    </section>
  );
}
