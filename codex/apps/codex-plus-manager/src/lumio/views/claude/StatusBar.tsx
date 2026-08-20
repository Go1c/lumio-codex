import { workspaceStatusCopy } from "../../claude/sync-status.ts";
import type { ClaudeProject, ClaudeSyncStatus } from "../../claude/types.ts";

export function StatusBar({
  sync,
  active,
}: {
  sync: ClaudeSyncStatus | null;
  active: ClaudeProject | null;
}) {
  return (
    <div className="lumio-claude-status">
      <span>{workspaceStatusCopy(sync)}</span>
      <span>{active ? `${active.user}@${active.host}` : ""}</span>
    </div>
  );
}
