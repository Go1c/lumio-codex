import type { HealthSnapshot } from "./types";
import { t } from "../../i18n";

const BOUNDARY_SECTIONS = [
  "desktop",
  "process",
  "watcher",
  "outbox",
  "transport",
  "stream",
  "cursor",
  "server",
] as const;

function formatValue(value: unknown): string {
  if (value === null || value === undefined) return "—";
  if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

function BoundarySection({
  title,
  data,
}: {
  title: string;
  data: Record<string, unknown>;
}) {
  const entries = Object.entries(data);
  return (
    <section className="diagnostics-section" aria-label={`${title} boundary`}>
      <h3 className="diagnostics-section-title">{title}</h3>
      {entries.length === 0 ? (
        <div className="diagnostics-empty">{t("logs.noData")}</div>
      ) : (
        <dl className="diagnostics-kv">
          {entries.map(([key, value]) => (
            <div key={key} style={{ display: "contents" }}>
              <dt>{key}</dt>
              <dd>{formatValue(value)}</dd>
            </div>
          ))}
        </dl>
      )}
    </section>
  );
}

export default function HealthView({
  health,
}: {
  health: HealthSnapshot | null;
}) {
  if (!health) {
    return (
      <div className="diagnostics-empty" role="status">
        Health snapshot is not available yet.
      </div>
    );
  }

  return (
    <section className="diagnostics-health" aria-label={t("logs.tabHealth")}>
      <div className="diagnostics-health-boundary" role="status">
        <strong>{t("logs.lastBoundary")}</strong>
        <div className="diagnostics-health-boundary-value">
          {health.lastProgressBoundary}
        </div>
        <div className="diagnostics-event-meta" style={{ marginTop: 8 }}>
          <span>project {health.projectRef}</span>
          <time dateTime={health.timestamp}>{health.timestamp}</time>
          <span>gen {health.connectionGeneration}</span>
        </div>
      </div>

      {BOUNDARY_SECTIONS.map((key) => (
        <BoundarySection key={key} title={key} data={health[key]} />
      ))}
    </section>
  );
}
