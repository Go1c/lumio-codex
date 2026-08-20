import { useEffect, useState } from "react";

import {
  formatCapturedClock,
  formatStatusBytes,
  serviceDisplayName,
} from "../../claude/remote-status.ts";
import { fetchClaudeServerStatus, fetchClaudeSessions } from "../../claude/session.ts";
import { dispatchClaude } from "../../claude/store.ts";
import type { ClaudeServerStatus, ClaudeSessionsSnapshot } from "../../claude/types.ts";

export function ServerStatusPane({ projectId }: { projectId: string }) {
  const [snapshot, setSnapshot] = useState<ClaudeServerStatus | null>(null);
  const [loading, setLoading] = useState(true);

  const load = () => {
    setLoading(true);
    void fetchClaudeServerStatus(projectId)
      .then((next) => setSnapshot(next))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    load();
  }, [projectId]);

  const host = snapshot?.host;
  const clock = snapshot?.capturedAt ? formatCapturedClock(snapshot.capturedAt) : "";

  return (
    <div className="lumio-claude-status-pane" aria-label="服务器状态">
      <div className="lumio-claude-status-toolbar">
        <span className="dim">{clock ? `采集于 ${clock}` : loading ? "正在读取…" : "尚未采集"}</span>
        <button className="lumio-button is-secondary" onClick={load} type="button">
          刷新
        </button>
      </div>
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
          onClick={() => dispatchClaude({ type: "set-stage-tab", tab: "terminal" })}
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
