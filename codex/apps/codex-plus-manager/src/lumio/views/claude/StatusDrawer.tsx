import { useEffect, useState, useSyncExternalStore } from "react";

import {
  formatCapturedClock,
  formatStatusBytes,
  serviceDisplayName,
} from "../../claude/remote-status.ts";
import {
  fetchClaudeServerStatus,
  fetchClaudeSessions,
  reinstallWorkspaceSync,
  resumeClaudeSync,
} from "../../claude/session.ts";
import { dispatchClaude, getClaudeState, subscribeClaudeStore } from "../../claude/store.ts";
import { SYNC_REINSTALL_LABEL, SYNC_RELAUNCH_LABEL, syncNeedsRecovery } from "../../claude/sync-status.ts";
import type {
  ClaudeServerStatus,
  ClaudeSessionsSnapshot,
  ClaudeState,
  ClaudeStatusDrawerPane,
} from "../../claude/types.ts";
import { ConflictsPane } from "./ConflictsPane.tsx";
import { liveSessionRows, nextStatusDrawerPane, sessionRowStatus, sessionTitleCopy } from "./status-copy.ts";

const EMPTY_CONFLICTS: ClaudeState["conflictsByProject"][string] = [];

export function StatusDrawer({ state: stateProp }: { state?: ClaudeState } = {}) {
  const stored = useSyncExternalStore(subscribeClaudeStore, getClaudeState, getClaudeState);
  const state = stateProp ?? stored;
  const pane = state.statusDrawer;
  if (pane === "closed") return null;

  const active =
    state.projects.find((project) => project.id === state.activeProjectId) ?? state.projects[0] ?? null;
  const conflicts = active ? (state.conflictsByProject[active.id] ?? EMPTY_CONFLICTS) : EMPTY_CONFLICTS;
  const rows = liveSessionRows(state.projects, state.sessionsByProject);
  const activeSessionId = active ? (state.activeSessionByProject[active.id] ?? null) : null;

  const open = (next: ClaudeStatusDrawerPane) => {
    dispatchClaude({ type: "set-status-drawer", pane: next });
  };

  return (
    <div className="lumio-claude-drawer">
      <div className="lumio-claude-drawer-tabs" onClick={() => open("closed")}>
        <button className={pane === "server" ? "is-on" : ""} onClick={(event) => { event.stopPropagation(); open(nextStatusDrawerPane(pane, "server")); }} type="button">服务器状态</button>
        <button className={pane === "sessions" ? "is-on" : ""} onClick={(event) => { event.stopPropagation(); open(nextStatusDrawerPane(pane, "sessions")); }} type="button">对话状态</button>
        <button className={pane === "conflicts" ? "is-on" : ""} onClick={(event) => { event.stopPropagation(); open(nextStatusDrawerPane(pane, "conflicts")); }} type="button">冲突</button>
        <button className="close" onClick={(event) => { event.stopPropagation(); open("closed"); }} type="button">收起 ✕</button>
      </div>
      {pane === "server" ? (
        active ? (
          <ServerStatusPane projectId={active.id} />
        ) : (
          <div className="lumio-claude-drawer-body">
            <p className="dim">还没有项目</p>
          </div>
        )
      ) : null}
      {pane === "sessions" ? (
        <div className="lumio-claude-drawer-body" aria-label="对话状态">
          {rows.length === 0 ? (
            <p className="dim">暂无对话。</p>
          ) : (
            <table className="lumio-claude-status-table">
              <thead>
                <tr>
                  <th>项目</th>
                  <th>标题</th>
                  <th>状态</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((row) => (
                  <tr key={`${row.projectId}:${row.session.id}`}>
                    <td>{row.projectName}</td>
                    <td>{sessionTitleCopy(row.session)}</td>
                    <td>
                      {sessionRowStatus(row.session, {
                        activeProjectId: state.activeProjectId,
                        activeSessionId,
                      })}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          {active ? (
            <section className="lumio-claude-status-card">
              <h3>这台服务器上的对话</h3>
              <SessionsPane projectId={active.id} />
            </section>
          ) : null}
        </div>
      ) : null}
      {pane === "conflicts" ? (
        active ? (
          <ConflictsPane conflicts={conflicts} projectId={active.id} />
        ) : (
          <div className="lumio-claude-drawer-body">
            <p>暂无冲突。远端和本机的改动不会被静默覆盖。</p>
          </div>
        )
      ) : null}
    </div>
  );
}

export function ServerStatusPane({ projectId }: { projectId: string }) {
  const state = useSyncExternalStore(subscribeClaudeStore, getClaudeState, getClaudeState);
  const [snapshot, setSnapshot] = useState<ClaudeServerStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [confirmReinstall, setConfirmReinstall] = useState(false);

  const load = () => {
    setLoading(true);
    void fetchClaudeServerStatus(projectId)
      .then((next) => setSnapshot(next))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    load();
    setConfirmReinstall(false);
  }, [projectId]);

  const host = snapshot?.host;
  const clock = snapshot?.capturedAt ? formatCapturedClock(snapshot.capturedAt) : "";
  const needsRecovery = syncNeedsRecovery(state.syncByProject[projectId] ?? null, snapshot);
  const servicesDown = snapshot?.services?.items.some((item) => !item.running) === true;
  const showRecovery = needsRecovery || servicesDown;

  return (
    <div className="lumio-claude-status-pane" aria-label="服务器状态">
      <div className="lumio-claude-status-toolbar">
        <span className="dim">{clock ? `采集于 ${clock}` : loading ? "正在读取…" : "尚未采集"}</span>
        <button className="lumio-button is-secondary" onClick={load} type="button">
          刷新
        </button>
        {showRecovery ? (
          <button
            className="lumio-button"
            onClick={() => void resumeClaudeSync(projectId).then(load)}
            type="button"
          >
            {SYNC_RELAUNCH_LABEL}
          </button>
        ) : null}
        {showRecovery ? (
          <button
            className="lumio-button is-secondary"
            onClick={() => setConfirmReinstall(true)}
            type="button"
          >
            {SYNC_REINSTALL_LABEL}
          </button>
        ) : null}
      </div>
      {confirmReinstall ? (
        <div className="lumio-claude-choice" role="dialog" aria-labelledby="lumio-claude-reinstall-title">
          <h3 id="lumio-claude-reinstall-title">已经装过同步组件</h3>
          <p className="dim">这台服务器上已经有同步组件。点重装会换成这一版并保持运行，不会当成失败。</p>
          <div className="lumio-claude-actions">
            <button
              className="lumio-button is-secondary"
              onClick={() => setConfirmReinstall(false)}
              type="button"
            >
              取消
            </button>
            <button
              className="lumio-button is-primary"
              onClick={() => {
                setConfirmReinstall(false);
                void reinstallWorkspaceSync(projectId).then(load);
              }}
              type="button"
            >
              {SYNC_REINSTALL_LABEL}
            </button>
          </div>
        </div>
      ) : null}
      {snapshot && !snapshot.ok && snapshot.error ? (
        <div className="lumio-claude-fail" role="alert">
          {snapshot.error.message}
        </div>
      ) : null}
      {host ? (
        <>
          <section className="lumio-claude-status-card">
            <h3>主机</h3>
            <div className="lumio-claude-metrics">
              <div>
                <strong>{host.cpu.usagePercent.toFixed(1)}%</strong>
                <span>CPU{host.cpu.cores != null ? ` · ${host.cpu.cores} 核` : ""}</span>
              </div>
              <div>
                <strong>{host.cpu.load1 != null ? host.cpu.load1.toFixed(2) : "—"}</strong>
                <span>1 分钟负载</span>
              </div>
              <div>
                <strong>{formatStatusBytes(host.memory.usedBytes)}</strong>
                <span>
                  内存 {formatStatusBytes(host.memory.totalBytes)} · {host.memory.usedPercent.toFixed(0)}%
                </span>
              </div>
            </div>
            <div className="lumio-claude-meter" aria-hidden="true">
              <i style={{ width: `${Math.min(host.memory.usedPercent, 100)}%` }} />
            </div>
            {host.hostname ? <p className="dim">{host.hostname}</p> : null}
          </section>
          <section className="lumio-claude-status-card">
            <h3>磁盘</h3>
            <table className="lumio-claude-status-table">
              <thead>
                <tr>
                  <th>挂载点</th>
                  <th>已用</th>
                  <th>总量</th>
                  <th>%</th>
                </tr>
              </thead>
              <tbody>
                {host.disks.map((disk) => (
                  <tr key={disk.mount}>
                    <td>
                      <code>{disk.mount}</code>
                    </td>
                    <td>{formatStatusBytes(disk.usedBytes)}</td>
                    <td>{formatStatusBytes(disk.totalBytes)}</td>
                    <td>{disk.usedPercent.toFixed(0)}%</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </section>
        </>
      ) : null}
      {snapshot?.services ? (
        <section className="lumio-claude-status-card">
          <h3>服务</h3>
          <table className="lumio-claude-status-table">
            <thead>
              <tr>
                <th>名称</th>
                <th>状态</th>
                <th>进程</th>
                <th>内存</th>
              </tr>
            </thead>
            <tbody>
              {snapshot.services.items.map((item) => (
                <tr key={item.key}>
                  <td>{item.displayName || serviceDisplayName(item.key)}</td>
                  <td>{item.running ? "运行中" : "未运行"}</td>
                  <td>{item.processCount}</td>
                  <td>{formatStatusBytes(item.memoryRssBytes)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      ) : null}
      {loading && !snapshot ? <p className="dim">正在读取服务器状态…</p> : null}
    </div>
  );
}

export function SessionsPane({ projectId }: { projectId: string }) {
  const [snapshot, setSnapshot] = useState<ClaudeSessionsSnapshot | null>(null);
  const [loading, setLoading] = useState(true);

  const load = () => {
    setLoading(true);
    void fetchClaudeSessions(projectId)
      .then((next) => setSnapshot(next))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    load();
  }, [projectId]);

  const clock = snapshot?.capturedAt ? formatCapturedClock(snapshot.capturedAt) : "";

  return (
    <div className="lumio-claude-status-pane" aria-label="对话状态">
      <div className="lumio-claude-status-toolbar">
        <span className="dim">
          {snapshot?.windows
            ? `${snapshot.windows.length} 个对话`
            : loading
              ? "正在读取…"
              : ""}
          {clock ? ` · ${clock}` : ""}
        </span>
        <button className="lumio-button is-secondary" onClick={load} type="button">
          刷新
        </button>
        <button
          className="lumio-button is-secondary"
          onClick={() => dispatchClaude({ type: "set-status-drawer", pane: "closed" })}
          type="button"
        >
          打开终端
        </button>
      </div>
      {snapshot && !snapshot.ok && snapshot.error ? (
        <div className="lumio-claude-fail" role="alert">
          {snapshot.error.message}
        </div>
      ) : null}
      {snapshot?.ok && snapshot.sessionExists === false ? (
        <p className="dim">暂无对话。</p>
      ) : null}
      {snapshot?.ok && snapshot.sessionExists && snapshot.windows.length === 0 ? (
        <p className="dim">暂无对话窗口。</p>
      ) : null}
      {snapshot && snapshot.windows.length > 0 ? (
        <section className="lumio-claude-status-card">
          <table className="lumio-claude-status-table">
            <thead>
              <tr>
                <th>#</th>
                <th>标题</th>
                <th>当前</th>
              </tr>
            </thead>
            <tbody>
              {snapshot.windows.map((window) => (
                <tr key={window.id}>
                  <td>{window.index}</td>
                  <td>{window.title}</td>
                  <td>{window.active ? "当前" : "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      ) : null}
      {loading && !snapshot ? <p className="dim">正在读取对话状态…</p> : null}
    </div>
  );
}
