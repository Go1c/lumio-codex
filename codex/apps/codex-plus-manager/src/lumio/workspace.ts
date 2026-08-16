import type { LumioPhase } from "./types.ts";

export type WorkspaceId = "codex" | "claude";

export const DEFAULT_WORKSPACE: WorkspaceId = "codex";

export function workspaceTabsVisible(phase: LumioPhase): boolean {
  return phase === "ready-online" || phase === "ready-offline";
}
