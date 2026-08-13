/** Typed facade for remote host status + project-scoped Claude sessions. */

export type RemoteMonitorError = {
  code: string;
  message: string;
};

export type CpuMetrics = {
  usagePercent: number;
  load1?: number;
  load5?: number;
  load15?: number;
  cores?: number;
};

export type MemoryMetrics = {
  totalBytes: number;
  usedBytes: number;
  availableBytes: number;
  usedPercent: number;
};

export type DiskMetrics = {
  mount: string;
  totalBytes: number;
  usedBytes: number;
  availableBytes: number;
  usedPercent: number;
};

export type HostMetrics = {
  hostname?: string;
  uptimeSeconds?: number;
  cpu: CpuMetrics;
  memory: MemoryMetrics;
  disks: DiskMetrics[];
};

export type ServiceItem = {
  key: string;
  displayName: string;
  running: boolean;
  processCount: number;
  cpuPercent: number;
  memoryRssBytes: number;
  pids: number[];
};

export type ServicesMetrics = {
  items: ServiceItem[];
  ourServicesMemoryRssBytes: number;
  ourServicesCpuPercent: number;
};

export type ServerStatusSnapshot = {
  projectId: string;
  sshHostAlias: string;
  capturedAt: string;
  ok: boolean;
  error?: RemoteMonitorError;
  host?: HostMetrics;
  services?: ServicesMetrics;
};

export type ClaudeSessionWindow = {
  index: number;
  id: string;
  name: string;
  title: string;
  active: boolean;
  paneCount: number;
  looksLikeClaude?: boolean;
};

export type ClaudeSessionsSnapshot = {
  projectId: string;
  sshHostAlias: string;
  tmuxSession: string;
  capturedAt: string;
  ok: boolean;
  error?: RemoteMonitorError;
  sessionExists: boolean;
  windows: ClaudeSessionWindow[];
  activeIndex: number | null;
};

export type InvokeFn = <T>(
  cmd: string,
  args?: Record<string, unknown>,
) => Promise<T>;

export function createRemoteMonitorClient(invokeFn: InvokeFn) {
  return {
    getServerStatus(projectId: string) {
      return invokeFn<ServerStatusSnapshot>("get_server_status", { projectId });
    },
    listClaudeSessions(projectId: string) {
      return invokeFn<ClaudeSessionsSnapshot>("list_claude_sessions", {
        projectId,
      });
    },
    switchClaudeSession(projectId: string, windowIndex: number) {
      return invokeFn<void>("switch_claude_session", {
        projectId,
        windowIndex,
      });
    },
    killClaudeSession(projectId: string, windowIndex: number) {
      return invokeFn<void>("kill_claude_session", {
        projectId,
        windowIndex,
      });
    },
  };
}

export type RemoteMonitorClient = ReturnType<typeof createRemoteMonitorClient>;

export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v < 10 && i > 0 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}
