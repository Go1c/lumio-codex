import { useState } from "react";
import { t } from "../../i18n";
import type { DiagnosticsClient } from "../../lib/diagnosticsApi";
import type { SupportBundlePreview } from "./types";

export default function SupportBundleView({
  projectId,
  client,
}: {
  projectId: string;
  client: DiagnosticsClient;
}) {
  const [preview, setPreview] = useState<SupportBundlePreview | null>(null);
  const [exportPath, setExportPath] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function loadPreview() {
    setBusy(true);
    setError(null);
    setExportPath(null);
    try {
      const result = await client.previewSupportBundle(projectId);
      setPreview(result);
    } catch (err) {
      setPreview(null);
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function onExport() {
    if (!preview) return;
    setBusy(true);
    setError(null);
    try {
      const result = await client.exportSupportBundle(projectId);
      setExportPath(result.path);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="diagnostics-support-bundle" aria-label={t("logs.tabSupportBundle")}>
      <p className="diagnostics-empty" style={{ padding: "0 0 12px" }}>
        Preview redaction summary before export. Export stays disabled until a
        preview has loaded.
      </p>

      <div className="diagnostics-actions">
        <button
          type="button"
          className="btn btn-secondary"
          onClick={loadPreview}
          disabled={busy || !projectId}
        >
          Load preview
        </button>
        <button
          type="button"
          className="btn btn-primary"
          onClick={onExport}
          disabled={busy || !preview}
        >
          Export support bundle
        </button>
      </div>

      {error && (
        <div className="diagnostics-error" role="alert">
          {error}
        </div>
      )}

      {preview && (
        <div className="diagnostics-preview" role="region" aria-label={t("logs.redactionPreview")}>
          <h3>{t("logs.redactionSummary")}</h3>
          <dl className="diagnostics-kv">
            <div style={{ display: "contents" }}>
              <dt>{t("logs.eventCount")}</dt>
              <dd>{preview.eventCount}</dd>
            </div>
            <div style={{ display: "contents" }}>
              <dt>{t("logs.timeRange")}</dt>
              <dd>
                {preview.timeRange.from ?? "—"} → {preview.timeRange.to ?? "—"}
              </dd>
            </div>
            <div style={{ display: "contents" }}>
              <dt>{t("logs.secretHits")}</dt>
              <dd>{preview.redactionSummary.secretHits}</dd>
            </div>
            <div style={{ display: "contents" }}>
              <dt>{t("logs.pathRedactions")}</dt>
              <dd>{preview.redactionSummary.pathRedactions}</dd>
            </div>
            <div style={{ display: "contents" }}>
              <dt>{t("logs.fieldsRemoved")}</dt>
              <dd>{preview.redactionSummary.fieldsRemoved}</dd>
            </div>
            <div style={{ display: "contents" }}>
              <dt>{t("logs.includesPaths")}</dt>
              <dd>{preview.includesPaths ? "yes" : "no"}</dd>
            </div>
          </dl>
        </div>
      )}

      {exportPath && (
        <div className="diagnostics-status" role="status">
          Exported to: {exportPath}
        </div>
      )}
    </section>
  );
}
