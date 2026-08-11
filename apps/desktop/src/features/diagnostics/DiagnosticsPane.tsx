import { useCallback, useEffect, useRef, useState } from "react";
import type { DiagnosticsClient } from "../../lib/diagnosticsApi";
import type { DiagnosticEvent, HealthSnapshot } from "./types";
import TimelineView from "./TimelineView";
import HealthView from "./HealthView";
import SelfTestView from "./SelfTestView";
import SupportBundleView from "./SupportBundleView";
import "./diagnostics.css";

type DiagnosticsTab = "timeline" | "health" | "self-test" | "support-bundle";

const TABS: Array<{ key: DiagnosticsTab; label: string }> = [
  { key: "timeline", label: "Timeline" },
  { key: "health", label: "Health" },
  { key: "self-test", label: "Self Test" },
  { key: "support-bundle", label: "Support Bundle" },
];

const DIAGNOSTICS_REFRESH_INTERVAL_MS = 5_000;

export default function DiagnosticsPane({
  projectId,
  client,
}: {
  projectId: string;
  client: DiagnosticsClient;
}) {
  const [tab, setTab] = useState<DiagnosticsTab>("timeline");
  const [events, setEvents] = useState<DiagnosticEvent[]>([]);
  const [health, setHealth] = useState<HealthSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [hasLoaded, setHasLoaded] = useState(false);
  const mounted = useRef(false);
  const refreshInFlight = useRef<number | null>(null);
  const requestGeneration = useRef(0);

  const refresh = useCallback(async () => {
    if (!projectId || refreshInFlight.current !== null) return;

    const generation = requestGeneration.current + 1;
    requestGeneration.current = generation;
    refreshInFlight.current = generation;
    if (mounted.current) {
      setLoading(true);
      setError(null);
    }

    try {
      const [nextEvents, nextHealth] = await Promise.all([
        client.listEvents({ projectId }),
        client.getHealth(projectId),
      ]);
      if (!mounted.current || generation !== requestGeneration.current) return;
      setEvents(nextEvents);
      setHealth(nextHealth);
      setHasLoaded(true);
    } catch (err) {
      if (!mounted.current || generation !== requestGeneration.current) return;
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      if (refreshInFlight.current === generation) {
        refreshInFlight.current = null;
      }
      if (mounted.current && generation === requestGeneration.current) {
        setLoading(false);
      }
    }
  }, [client, projectId]);

  useEffect(() => {
    mounted.current = true;
    requestGeneration.current += 1;
    refreshInFlight.current = null;
    setEvents([]);
    setHealth(null);
    setError(null);
    setLoading(false);
    setHasLoaded(false);

    let disposed = false;
    let refreshTimer: number | undefined;

    async function poll() {
      await refresh();
      if (!disposed) {
        refreshTimer = window.setTimeout(
          poll,
          DIAGNOSTICS_REFRESH_INTERVAL_MS,
        );
      }
    }

    if (projectId) void poll();

    return () => {
      disposed = true;
      mounted.current = false;
      requestGeneration.current += 1;
      refreshInFlight.current = null;
      if (refreshTimer !== undefined) window.clearTimeout(refreshTimer);
    };
  }, [projectId, refresh]);

  return (
    <section className="diagnostics-pane" aria-label="Logs and diagnostics">
      <div className="diagnostics-tabs" role="tablist" aria-label="Diagnostics views">
        {TABS.map((item) => (
          <button
            key={item.key}
            type="button"
            role="tab"
            aria-selected={tab === item.key}
            className={tab === item.key ? "active" : ""}
            onClick={() => setTab(item.key)}
          >
            {item.label}
          </button>
        ))}
        <button
          type="button"
          className="btn btn-secondary"
          style={{ marginLeft: "auto", alignSelf: "center", marginRight: 8 }}
          onClick={() => void refresh()}
          disabled={loading}
          aria-busy={loading}
        >
          {loading ? "Refreshing..." : "Refresh"}
        </button>
      </div>

      <div className="diagnostics-body">
        {error && (
          <div className="diagnostics-error" role="alert">
            {error}
          </div>
        )}

        {tab === "timeline" && (
          <TimelineView
            events={events}
            loading={loading}
            hasLoaded={hasLoaded}
          />
        )}
        {tab === "health" && <HealthView health={health} />}
        {tab === "self-test" && <SelfTestView client={client} />}
        {tab === "support-bundle" && (
          <SupportBundleView projectId={projectId} client={client} />
        )}
      </div>
    </section>
  );
}
