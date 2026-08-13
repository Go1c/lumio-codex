import { useMemo, useState } from "react";
import { t } from "../../i18n";
import { filterEvents } from "./filter";
import type { DiagnosticEvent, DiagnosticLevel, TimelineFilter } from "./types";

const LEVELS: Array<DiagnosticLevel | ""> = [
  "",
  "trace",
  "debug",
  "info",
  "warn",
  "error",
];

export default function TimelineView({
  events,
  loading,
  hasLoaded,
}: {
  events: readonly DiagnosticEvent[];
  loading: boolean;
  hasLoaded: boolean;
}) {
  const [level, setLevel] = useState<DiagnosticLevel | "">("");
  const [component, setComponent] = useState("");
  const [eventName, setEventName] = useState("");
  const [runId, setRunId] = useState("");

  const filter: TimelineFilter = { level, component, eventName, runId };
  const visible = useMemo(
    () => filterEvents(events, filter),
    [events, level, component, eventName, runId],
  );
  const hasActiveFilter = Boolean(level || component || eventName || runId);

  let emptyMessage = t("logs.noMatches");
  if (loading && !hasLoaded) {
    emptyMessage = t("logs.loading");
  } else if (!hasLoaded) {
    emptyMessage = t("logs.unavailable");
  } else if (events.length === 0) {
    emptyMessage = t("logs.empty");
  } else if (!hasActiveFilter) {
    emptyMessage = t("logs.empty");
  }

  return (
    <section className="diagnostics-timeline" aria-label={t("logs.tabTimeline")}>
      <div className="diagnostics-toolbar">
        <div className="diagnostics-field">
          <label htmlFor="diagnostics-level">{t("logs.level")}</label>
          <select
            id="diagnostics-level"
            value={level}
            onChange={(e) => setLevel(e.target.value as DiagnosticLevel | "")}
          >
            {LEVELS.map((value) => (
              <option key={value || "all"} value={value}>
                {value || t("logs.allLevels")}
              </option>
            ))}
          </select>
        </div>
        <div className="diagnostics-field">
          <label htmlFor="diagnostics-component">{t("logs.component")}</label>
          <input
            id="diagnostics-component"
            value={component}
            onChange={(e) => setComponent(e.target.value)}
            placeholder="e.g. transport"
          />
        </div>
        <div className="diagnostics-field">
          <label htmlFor="diagnostics-event-name">{t("logs.eventName")}</label>
          <input
            id="diagnostics-event-name"
            value={eventName}
            onChange={(e) => setEventName(e.target.value)}
            placeholder="e.g. workspace.ack.confirmed"
          />
        </div>
        <div className="diagnostics-field">
          <label htmlFor="diagnostics-run-id">{t("logs.runId")}</label>
          <input
            id="diagnostics-run-id"
            value={runId}
            onChange={(e) => setRunId(e.target.value)}
            placeholder="run id"
          />
        </div>
      </div>

      {visible.length === 0 ? (
        <div className="diagnostics-empty" role="status">
          {emptyMessage}
        </div>
      ) : (
        <ul className="diagnostics-event-list">
          {visible.map((event, index) => (
            <li
              className="diagnostics-event"
              key={`${event.timestamp}-${event.eventName}-${event.traceId}-${index}`}
            >
              <div className="diagnostics-event-meta">
                <span
                  className={`diagnostics-event-level diagnostics-event-level-${event.level}`}
                >
                  {event.level}
                </span>
                <time dateTime={event.timestamp}>{event.timestamp}</time>
                <span>{event.component}</span>
                <span>{event.eventName}</span>
                <span title={event.runId}>run {event.runId.slice(0, 8)}</span>
              </div>
              <p className="diagnostics-event-message">{event.message}</p>
              {Object.keys(event.fields).length > 0 && (
                <details className="diagnostics-event-fields">
                  <summary>{t("logs.fields", { n: Object.keys(event.fields).length })}</summary>
                  <dl>
                    {Object.entries(event.fields).map(([key, value]) => (
                      <div key={key}>
                        <dt>{key}</dt>
                        <dd>
                          <code>{formatFieldValue(value)}</code>
                        </dd>
                      </div>
                    ))}
                  </dl>
                </details>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function formatFieldValue(value: unknown): string {
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}
