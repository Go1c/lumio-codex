import { useCallback, useEffect, useRef, useState } from "react";
import { t } from "../../i18n";
import type {
  ClaudeSessionsSnapshot,
  RemoteMonitorClient,
} from "../../lib/remoteMonitorApi";
import "./remote-monitor.css";

const POLL_MS = 15_000;
const REFRESH_DEBOUNCE_MS = 2_000;

type Props = {
  projectId: string;
  client: RemoteMonitorClient;
  onRequestTerminalTab: () => void;
};

export default function ClaudeSessionsPane({
  projectId,
  client,
  onRequestTerminalTab,
}: Props) {
  const [snapshot, setSnapshot] = useState<ClaudeSessionsSnapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [actionError, setActionError] = useState<string | null>(null);
  const [busyIndex, setBusyIndex] = useState<number | null>(null);
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
        const next = await client.listClaudeSessions(projectId);
        if (cancelled.current || gen !== generation.current) return;
        setSnapshot(next);
      } catch (error) {
        if (cancelled.current || gen !== generation.current) return;
        setSnapshot({
          projectId,
          sshHostAlias: "",
          tmuxSession: "",
          capturedAt: new Date().toISOString(),
          ok: false,
          sessionExists: false,
          windows: [],
          activeIndex: null,
          error: {
            code: "ssh_failed",
            message:
              error instanceof Error ? error.message : t("sessions.listFailed"),
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
    setActionError(null);

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

  async function onSwitch(index: number) {
    setActionError(null);
    setBusyIndex(index);
    try {
      await client.switchClaudeSession(projectId, index);
      onRequestTerminalTab();
      await load(true);
    } catch (error) {
      setActionError(
        error instanceof Error ? error.message : t("sessions.switchFailed"),
      );
    } finally {
      setBusyIndex(null);
    }
  }

  async function onKill(index: number, title: string) {
    if (
      !confirm(
        t("sessions.killConfirm", { name: title, index }),
      )
    ) {
      return;
    }
    setActionError(null);
    setBusyIndex(index);
    try {
      await client.killClaudeSession(projectId, index);
      await load(true);
    } catch (error) {
      setActionError(error instanceof Error ? error.message : t("sessions.killFailed"));
    } finally {
      setBusyIndex(null);
    }
  }

  return (
    <div className="remote-monitor-pane" aria-label={t("sessions.title")}>
      <div className="remote-monitor-toolbar">
        {snapshot?.tmuxSession ? (
          <span className="remote-monitor-badge">{snapshot.tmuxSession}</span>
        ) : null}
        <span className="muted">
          {snapshot?.windows
            ? t("sessions.windowCount", { n: snapshot.windows.length })
            : loading
              ? t("common.loading")
              : ""}
        </span>
        <span className="muted">
          {snapshot?.capturedAt
            ? t("sessions.capturedAt", {
                time: new Date(snapshot.capturedAt).toLocaleTimeString(),
              })
            : ""}
        </span>
        <button
          type="button"
          className="btn btn-secondary"
          onClick={() => void load(false)}
        >
          {t("sessions.refresh")}
        </button>
      </div>

      {snapshot && !snapshot.ok && snapshot.error ? (
        <div className="remote-monitor-error" role="alert">
          <strong>{snapshot.error.code}</strong>: {snapshot.error.message}
        </div>
      ) : null}

      {actionError ? (
        <div className="remote-monitor-error" role="alert">
          {actionError}
        </div>
      ) : null}

      {snapshot?.ok && snapshot.sessionExists === false ? (
        <p className="remote-monitor-empty">
          {t("sessions.noSession")}
        </p>
      ) : null}

      {snapshot?.ok && snapshot.sessionExists && snapshot.windows.length === 0 ? (
        <p className="remote-monitor-empty">
          {t("sessions.empty")}
        </p>
      ) : null}

      {snapshot && snapshot.windows.length > 0 ? (
        <section className="remote-monitor-card">
          <table className="remote-monitor-table">
            <thead>
              <tr>
                <th>#</th>
                <th>{t("sessions.colTitle")}</th>
                <th>{t("sessions.colActive")}</th>
                <th>{t("sessions.colActions")}</th>
              </tr>
            </thead>
            <tbody>
              {snapshot.windows.map((win) => (
                <tr key={win.id}>
                  <td>{win.index}</td>
                  <td>{win.title}</td>
                  <td>
                    <span
                      className={
                        win.active
                          ? "remote-monitor-pill"
                          : "remote-monitor-pill inactive"
                      }
                    >
                      {win.active ? t("sessions.active") : "—"}
                    </span>
                  </td>
                  <td>
                    <div className="remote-monitor-actions">
                      <button
                        type="button"
                        className="btn-switch"
                        disabled={busyIndex !== null}
                        onClick={() => void onSwitch(win.index)}
                      >
                        {t("sessions.switch")}
                      </button>
                      <button
                        type="button"
                        className="btn-kill"
                        disabled={busyIndex !== null}
                        onClick={() => void onKill(win.index, win.title)}
                      >
                        {t("sessions.kill")}
                      </button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      ) : null}

      {loading && !snapshot ? (
        <p className="remote-monitor-empty">{t("sessions.loading")}</p>
      ) : null}
    </div>
  );
}
