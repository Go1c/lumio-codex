import { syncErrorCopy } from "./api.ts";
import type { ClaudeProject, ClaudeServerStatus, ClaudeSyncStatus } from "./types.ts";

export type WorkspaceStatusTone = "ok" | "warn" | "bad" | "plain";

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

export function reconcileSyncWithRemote(
  sync: ClaudeSyncStatus | null,
  snapshot: ClaudeServerStatus | null,
): ClaudeSyncStatus | null {
  if (!sync) return sync;
  if (!snapshot?.ok || !snapshot.services) return sync;
  const remoteSync = snapshot.services.items.find((item) => item.key === "sync");
  if (!remoteSync || remoteSync.running) return sync;
  if (sync.state === "conflicts" || sync.state === "offline") return sync;
  if (sync.state === "fail" && sync.errorCode && sync.errorCode !== "SYNC_REMOTE_NOT_RUNNING") {
    return sync;
  }
  return {
    ...sync,
    state: "fail",
    errorCode: "SYNC_REMOTE_NOT_RUNNING",
  };
}

export function workspaceStatusAppearance(
  sync: ClaudeSyncStatus | null,
  remote?: ClaudeServerStatus | null,
): { copy: string; tone: WorkspaceStatusTone } {
  const effective = reconcileSyncWithRemote(sync, remote ?? null);
  if (!effective || effective.state === "idle") return { copy: "同步未运行", tone: "bad" };
  if (effective.state === "fail") return { copy: syncErrorCopy(effective.errorCode), tone: "bad" };
  if (effective.state === "conflicts") {
    return { copy: `${effective.conflicts} 个冲突`, tone: "warn" };
  }
  if (effective.state === "running") {
    if (effective.filesTotal > 0) {
      return {
        copy: `同步运行中 · ${effective.filesDone} / ${effective.filesTotal}`,
        tone: "plain",
      };
    }
    return { copy: "同步运行中", tone: "plain" };
  }
  if (effective.state === "synced") return { copy: "已同步 · 文件与远端一致", tone: "ok" };
  if (effective.state === "offline") return { copy: "离线 · 本机目录可用", tone: "warn" };
  return { copy: "同步未运行", tone: "bad" };
}

export function workspaceStatusCopy(
  sync: ClaudeSyncStatus | null,
  remote?: ClaudeServerStatus | null,
): string {
  return workspaceStatusAppearance(sync, remote).copy;
}
