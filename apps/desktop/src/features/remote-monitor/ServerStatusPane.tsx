import { useCallback, useEffect, useRef, useState } from "react";
import {
  formatBytes,
  type RemoteMonitorClient,
  type ServerStatusSnapshot,
} from "../../lib/remoteMonitorApi";
import "./remote-monitor.css";

const POLL_MS = 20_000;
const REFRESH_DEBOUNCE_MS = 2_000;

type Props = {
  projectId: string;
  client: RemoteMonitorClient;
};

function barClass(percent: number): string {
  if (percent >= 90) return "remote-monitor-bar danger";
  if (percent >= 75) return "remote-monitor-bar warn";
  return "remote-monitor-bar";
}

export default function ServerStatusPane({ projectId, client }: Props) {
  const [snapshot, setSnapshot] = useState<ServerStatusSnapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const generation = useRef(0);
  const lastRefresh = useRef(0);
  const cancelled = useRef(false);

  const load = useCallback(
    async (force: boolean) => {
      const now = Date.now();
      if (!force && now - lastRefresh.current < REFRESH_DEBOUNCE_MS) return;
      lastRefresh.current = now;
      const gen = generation.current;
      try {
        const next = await client.getServerStatus(projectId);
        if (cancelled.current || gen !== generation.current) return;
        setSnapshot(next);
      } catch (error) {
        if (cancelled.current || gen !== generation.current) return;
        setSnapshot({
          projectId,
          sshHostAlias: "",
          capturedAt: new Date().toISOString(),
          ok: false,
          error: {
            code: "ssh_failed",
            message:
              error instanceof Error ? error.message : "Failed to load status",
          },
        });
      } finally {
        if (!cancelled.current && gen === generation.current) {
          setLoading(false);
        }
      }
    },
    [client, projectId],
  );

  useEffect(() => {
    cancelled.current = false;
    generation.current += 1;
    setLoading(true);
    setSnapshot(null);

    let timer: ReturnType<typeof setTimeout> | undefined;
    const tick = async () => {
      await load(true);
      if (!cancelled.current) {
        timer = window.setTimeout(tick, POLL_MS);
      }
    };
    void tick();

    return () => {
      cancelled.current = true;
      generation.current += 1;
      if (timer !== undefined) clearTimeout(timer);
    };
  }, [projectId, load]);

  const host = snapshot?.host;
  const services = snapshot?.services;
  const ourMem = services?.ourServicesMemoryRssBytes ?? 0;
  const hostUsed = host?.memory.usedBytes ?? 0;

  return (
    <div className="remote-monitor-pane" aria-label="Server status">
      <div className="remote-monitor-toolbar">
        <span className="muted">
          {snapshot?.capturedAt
            ? `Last updated ${new Date(snapshot.capturedAt).toLocaleTimeString()}`
            : loading
              ? "Loading…"
              : "Not yet"}
        </span>
        <button
          type="button"
          className="btn btn-secondary"
          onClick={() => void load(false)}
        >
          Refresh
        </button>
        {snapshot?.sshHostAlias ? (
          <span className="remote-monitor-badge">{snapshot.sshHostAlias}</span>
        ) : null}
      </div>

      {snapshot && !snapshot.ok && snapshot.error ? (
        <div className="remote-monitor-error" role="alert">
          <strong>{snapshot.error.code}</strong>: {snapshot.error.message}
          <div className="muted" style={{ marginTop: 4 }}>
            Check SSH config and project host connectivity.
          </div>
        </div>
      ) : null}

      {host ? (
        <>
          <section className="remote-monitor-card" aria-label="Host metrics">
            <h3>Host</h3>
            <div className="remote-monitor-metrics">
              <div className="remote-monitor-metric">
                <strong>{host.cpu.usagePercent.toFixed(1)}%</strong>
                <span>
                  CPU
                  {host.cpu.cores != null ? ` · ${host.cpu.cores} cores` : ""}
                </span>
              </div>
              <div className="remote-monitor-metric">
                <strong>
                  {host.cpu.load1 != null ? host.cpu.load1.toFixed(2) : "—"}
                </strong>
                <span>Load (1m)</span>
              </div>
              <div className="remote-monitor-metric">
                <strong>{formatBytes(host.memory.usedBytes)}</strong>
                <span>
                  Memory of {formatBytes(host.memory.totalBytes)} (
                  {host.memory.usedPercent.toFixed(0)}%)
                </span>
              </div>
            </div>
            <div
              className={barClass(host.memory.usedPercent)}
              aria-hidden="true"
            >
              <i style={{ width: `${Math.min(host.memory.usedPercent, 100)}%` }} />
            </div>
            {host.hostname || host.uptimeSeconds != null ? (
              <div className="muted">
                {host.hostname ? host.hostname : ""}
                {host.hostname && host.uptimeSeconds != null ? " · " : ""}
                {host.uptimeSeconds != null
                  ? `uptime ${Math.floor(host.uptimeSeconds / 3600)}h`
                  : ""}
              </div>
            ) : null}
          </section>

          <section className="remote-monitor-card" aria-label="Disks">
            <h3>Disk</h3>
            <table className="remote-monitor-table">
              <thead>
                <tr>
                  <th>Mount</th>
                  <th>Used</th>
                  <th>Total</th>
                  <th>%</th>
                </tr>
              </thead>
              <tbody>
                {host.disks.map((d) => (
                  <tr key={d.mount}>
                    <td>
                      <code>{d.mount}</code>
                    </td>
                    <td>{formatBytes(d.usedBytes)}</td>
                    <td>{formatBytes(d.totalBytes)}</td>
                    <td>{d.usedPercent.toFixed(0)}%</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </section>
        </>
      ) : null}

      {services ? (
        <>
          <section className="remote-monitor-card" aria-label="Our services">
            <h3>Our services</h3>
            <div className="remote-monitor-highlight">
              Our services memory: {formatBytes(ourMem)}
              {hostUsed > 0 ? (
                <span className="muted"> / host used {formatBytes(hostUsed)}</span>
              ) : null}
            </div>
            <table className="remote-monitor-table">
              <thead>
                <tr>
                  <th>Service</th>
                  <th>Status</th>
                  <th>Procs</th>
                  <th>RSS</th>
                  <th>CPU%</th>
                </tr>
              </thead>
              <tbody>
                {services.items
                  .filter((i) => i.key === "fns-agent" || i.key === "fns-server")
                  .map((item) => (
                    <tr key={item.key}>
                      <td>{item.displayName}</td>
                      <td>{item.running ? "Running" : "Not running"}</td>
                      <td>{item.processCount}</td>
                      <td>{formatBytes(item.memoryRssBytes)}</td>
                      <td>{item.cpuPercent.toFixed(1)}</td>
                    </tr>
                  ))}
              </tbody>
            </table>
          </section>

          <section
            className="remote-monitor-card"
            aria-label="Claude processes on host"
          >
            <h3>Claude processes (host)</h3>
            {(() => {
              const claude = services.items.find((i) => i.key === "claude");
              if (!claude || !claude.running) {
                return (
                  <p className="remote-monitor-empty">
                    No claude processes detected on this host. Dialogue-level
                    list is under Claude Sessions.
                  </p>
                );
              }
              return (
                <p>
                  {claude.processCount} process
                  {claude.processCount === 1 ? "" : "es"} ·{" "}
                  {formatBytes(claude.memoryRssBytes)} RSS
                </p>
              );
            })()}
          </section>
        </>
      ) : null}

      {loading && !snapshot ? (
        <p className="remote-monitor-empty">Loading server status…</p>
      ) : null}
    </div>
  );
}
