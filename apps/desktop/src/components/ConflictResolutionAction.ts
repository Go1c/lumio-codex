import type {
  ConflictChoice,
  ConflictView,
  ResolutionReceipt,
} from "./ConflictPaneContent";
import type { ConflictControlIdentity } from "./ConflictRequestScope";

type InvokeCommand = <T>(
  command: string,
  args?: Record<string, unknown>,
) => Promise<T>;

export type ConflictResolutionResult =
  | { ok: true; receipt: ResolutionReceipt }
  | { ok: false; error: unknown };

export async function runConflictResolution({
  invokeCommand,
  projectId,
  identity,
  conflict,
  choice,
  refresh,
}: {
  invokeCommand: InvokeCommand;
  projectId: string;
  identity: ConflictControlIdentity;
  conflict: ConflictView;
  choice: ConflictChoice;
  refresh: () => Promise<void>;
}): Promise<ConflictResolutionResult> {
  let result: ConflictResolutionResult;
  try {
    const receipt = await invokeCommand<ResolutionReceipt>(
      "resolve_sync_conflict",
      {
        projectId,
        identity,
        input: {
          conflictId: conflict.conflictId,
          conflictRevision: conflict.conflictRevision,
          choice,
        },
      },
    );
    result = { ok: true, receipt };
  } catch (error) {
    result = { ok: false, error };
  }

  await refresh();
  return result;
}
