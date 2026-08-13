import type {
  ClaudeSessionsSnapshot,
  ClaudeSessionWindow,
  RemoteMonitorClient,
  ServerStatusSnapshot,
} from "./remoteMonitorApi";

/**
 * In-memory host monitor for browser dev mode and tests.
 *
 * Mirrors the shapes `remote_monitor.rs` returns, including the failure shape,
 * so the panes can be exercised without a server.
 */
export interface MockRemoteMonitorSeed {
  sshHostAlias?: string;
  tmuxSession?: string;
  /** Report the host as unreachable instead of returning metrics. */
  unreachable?: boolean;
  /** Drive the amber/red thresholds in the bars. */
  cpuPercent?: number;
  memoryPercent?: number;
  diskPercent?: number;
  windows?: ClaudeSessionWindow[];
}

const GIB = 1024 ** 3;

function defaultWindows(): ClaudeSessionWindow[] {
  return [
    {
      index: 0,
      id: "@0",
      name: "claude",
      title: "claude — 重构同步引擎",
      active: true,
      paneCount: 1,
      looksLikeClaude: true,
    },
    {
      index: 1,
      id: "@1",
      name: "shell",
      title: "bash",
      active: false,
      paneCount: 2,
      looksLikeClaude: false,
    },
  ];
}

export function createMockRemoteMonitorClient(
  seed: MockRemoteMonitorSeed = {},
): RemoteMonitorClient {
  const sshHostAlias = seed.sshHostAlias ?? "root@43.156.20.8";
  const tmuxSession = seed.tmuxSession ?? "cchaven-my-project";
  let windows = seed.windows ? [...seed.windows] : defaultWindows();

  const unreachable = () => ({
    code: "unreachable",
    message: "无法连接到服务器。",
  });

  return {
    async getServerStatus(id: string): Promise<ServerStatusSnapshot> {
      const capturedAt = new Date().toISOString();
      if (seed.unreachable) {
        return {
          projectId: id,
          sshHostAlias,
          capturedAt,
          ok: false,
          error: unreachable(),
        };
      }
      const memoryPercent = seed.memoryPercent ?? 62;
      const diskPercent = seed.diskPercent ?? 41;
      return {
        projectId: id,
        sshHostAlias,
        capturedAt,
        ok: true,
        host: {
          hostname: "cchaven-demo",
          uptimeSeconds: 372_400,
          cpu: {
            usagePercent: seed.cpuPercent ?? 23,
            load1: 0.42,
            load5: 0.55,
            load15: 0.61,
            cores: 4,
          },
          memory: {
            totalBytes: 8 * GIB,
            usedBytes: Math.round((8 * GIB * memoryPercent) / 100),
            availableBytes: Math.round((8 * GIB * (100 - memoryPercent)) / 100),
            usedPercent: memoryPercent,
          },
          disks: [
            {
              mount: "/",
              totalBytes: 80 * GIB,
              usedBytes: Math.round((80 * GIB * diskPercent) / 100),
              availableBytes: Math.round((80 * GIB * (100 - diskPercent)) / 100),
              usedPercent: diskPercent,
            },
          ],
        },
        services: {
          items: [
            {
              key: "fns-server",
              displayName: "同步服务",
              running: true,
              processCount: 1,
              cpuPercent: 1.4,
              memoryRssBytes: 96 * 1024 * 1024,
              pids: [1201],
            },
            {
              key: "fns-agent",
              displayName: "同步代理",
              running: true,
              processCount: 1,
              cpuPercent: 0.6,
              memoryRssBytes: 48 * 1024 * 1024,
              pids: [1288],
            },
          ],
          ourServicesMemoryRssBytes: 144 * 1024 * 1024,
          ourServicesCpuPercent: 2,
        },
      };
    },

    async listClaudeSessions(id: string): Promise<ClaudeSessionsSnapshot> {
      const capturedAt = new Date().toISOString();
      if (seed.unreachable) {
        return {
          projectId: id,
          sshHostAlias,
          tmuxSession,
          capturedAt,
          ok: false,
          error: unreachable(),
          sessionExists: false,
          windows: [],
          activeIndex: null,
        };
      }
      return {
        projectId: id,
        sshHostAlias,
        tmuxSession,
        capturedAt,
        ok: true,
        sessionExists: windows.length > 0,
        windows: windows.map((window) => ({ ...window })),
        activeIndex: windows.find((window) => window.active)?.index ?? null,
      };
    },

    async switchClaudeSession(_id: string, windowIndex: number): Promise<void> {
      windows = windows.map((window) => ({
        ...window,
        active: window.index === windowIndex,
      }));
    },

    async killClaudeSession(_id: string, windowIndex: number): Promise<void> {
      windows = windows.filter((window) => window.index !== windowIndex);
      if (windows.length > 0 && !windows.some((window) => window.active)) {
        windows[0] = { ...windows[0], active: true };
      }
    },
  };
}
