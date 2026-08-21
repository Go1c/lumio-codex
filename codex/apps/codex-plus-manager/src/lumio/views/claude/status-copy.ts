import { DEFAULT_SESSION_TITLE } from "../../claude/session-title.ts";
import type {
  ClaudeChatSession,
  ClaudeLoginPhase,
  ClaudeStatusDrawerPane,
  ClaudeSyncStatus,
  ClaudeWorkspacePhase,
} from "../../claude/types.ts";

export function nextStatusDrawerPane(
  current: ClaudeStatusDrawerPane,
  requested: ClaudeStatusDrawerPane,
): ClaudeStatusDrawerPane {
  if (requested === "closed") return "closed";
  return current === requested ? "closed" : requested;
}

export function conversationCountCopy(total: number, running: number): string {
  return running > 0 ? `对话 ${total} · ${running} 在跑` : `对话 ${total}`;
}

export function collectSessions(sessionsByProject: Record<string, ClaudeChatSession[]>): ClaudeChatSession[] {
  return Object.values(sessionsByProject).flat();
}

export function readyStatusCopy(
  phase: ClaudeWorkspacePhase | undefined,
  sync: ClaudeSyncStatus | null,
): { tone: "ok" | "warn" | "bad" | "plain"; label: string } {
  if (phase === "init") return { tone: "plain", label: "正在准备" };
  if (phase === "resume") return { tone: "plain", label: "正在连接" };
  if (phase === "offline" || sync?.state === "offline") return { tone: "bad", label: "离线" };
  return { tone: "ok", label: "已就绪" };
}

function loginLabel(phase: ClaudeLoginPhase | undefined): string {
  if (phase === "logged-in") return "已登录";
  if (phase === "logging-in") return "正在登录";
  if (phase === "expired") return "登录已过期";
  return "未登录";
}

export function claudeVersionLoginCopy(
  cli: { version: string | null; latest: string | null } | null | undefined,
  login: { phase: ClaudeLoginPhase } | null | undefined,
): string {
  const version = cli?.version;
  const suffix = loginLabel(login?.phase);
  return version ? `Claude ${version} · ${suffix}` : `Claude · ${suffix}`;
}

export function updateNudgeCopy(
  cli: { version: string | null; latest: string | null } | null | undefined,
): string | null {
  if (!cli?.version || !cli.latest || cli.version === cli.latest) return null;
  return `有新版 ${cli.latest} · 升级`;
}

export function hostResourceCopy(
  host: { cpu: { usagePercent: number }; memory: { usedPercent: number } } | null | undefined,
): string | null {
  if (!host) return null;
  return `CPU ${host.cpu.usagePercent.toFixed(0)}% · 内存 ${host.memory.usedPercent.toFixed(0)}%`;
}

export function conflictFlagCopy(count: number): string | null {
  return count > 0 ? `冲突 ${count}` : null;
}

export function liveSessionRows(
  projects: { id: string; name: string }[],
  sessionsByProject: Record<string, ClaudeChatSession[]>,
): { projectId: string; projectName: string; session: ClaudeChatSession }[] {
  const nameById = new Map(projects.map((project) => [project.id, project.name]));
  const rows: { projectId: string; projectName: string; session: ClaudeChatSession }[] = [];
  for (const [projectId, sessions] of Object.entries(sessionsByProject)) {
    const projectName = nameById.get(projectId) ?? projectId;
    for (const session of sessions) {
      rows.push({ projectId, projectName, session });
    }
  }
  return rows;
}

export function sessionTitleCopy(session: ClaudeChatSession): string {
  return session.titleLocked && session.title ? session.title : DEFAULT_SESSION_TITLE;
}

export function sessionRowStatus(
  session: ClaudeChatSession,
  active: { activeProjectId: string | null; activeSessionId: string | null | undefined },
): "正在跑" | "当前" | "后台" {
  if (session.running) return "正在跑";
  if (session.projectId === active.activeProjectId && session.id === active.activeSessionId) return "当前";
  return "后台";
}
