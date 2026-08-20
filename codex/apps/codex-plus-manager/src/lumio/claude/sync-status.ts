import { syncErrorCopy } from "./api.ts";
import type { ClaudeProject, ClaudeSyncStatus } from "./types.ts";

export function projectsToResume(
  projects: ClaudeProject[],
  activeProjectId: string | null,
): ClaudeProject[] {
  const active = projects.find((project) => project.id === activeProjectId) ?? projects[0];
  return active ? [active] : [];
}

export async function resumeSavedProjects(
  projects: ClaudeProject[],
  activeProjectId: string | null,
  resume: (projectId: string) => Promise<void>,
): Promise<void> {
  for (const project of projectsToResume(projects, activeProjectId)) {
    await resume(project.id);
  }
}

export function workspaceStatusCopy(sync: ClaudeSyncStatus | null): string {
  if (!sync || sync.state === "idle") return "同步未运行";
  if (sync.state === "fail") return syncErrorCopy(sync.errorCode);
  if (sync.state === "conflicts") return `${sync.conflicts} 个冲突`;
  if (sync.state === "running") {
    if (sync.filesTotal > 0) {
      return `同步运行中 · ${sync.filesDone} / ${sync.filesTotal}`;
    }
    return "同步运行中";
  }
  if (sync.state === "synced") return "已同步 · 文件与远端一致";
  if (sync.state === "offline") return "离线 · 本机目录可用";
  return "同步未运行";
}
