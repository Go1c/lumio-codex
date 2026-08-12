import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  ConflictPaneContent,
  type ConflictChoice,
  type ConflictOperation,
  type ConflictView,
  type ResolutionReceipt,
} from "./ConflictPaneContent";
import {
  ConflictRequestScope,
  type ConflictControlIdentity,
} from "./ConflictRequestScope";
import { runConflictResolution } from "./ConflictResolutionAction";

const POLL_INTERVAL_MS = 2000;
const deferredCleanupFailures = new Map<string, string>();

function stableFailureLabel(code: string) {
  const labels: Record<string, string> = {
    state_corrupt: "The saved sync state is damaged",
    conflict_unavailable: "This conflict no longer exists",
    conflict_revision_stale: "This conflict changed before the decision was applied",
    conflict_resolution_changed: "A different resolution is already pending",
    conflict_waiting_blobs: "Required file content is still downloading",
    conflict_automatic_resolution_pending: "Automatic resolution is still pending",
    conflict_resolution_pending: "A resolution is already pending",
    conflict_refresh_required: "The conflict must be refreshed",
    conflict_selected_side_deleted: "The selected version has been deleted",
    merge_file_required: "The edited resolution target is not a file",
    merge_content_unavailable: "The edited resolution content is unavailable",
    conflict_request_unavailable: "This resolution request is no longer available",
    conflict_request_changed: "This request ID was already used for another resolution",
    request_cancelled: "The resolution was cancelled before it was sent",
    request_timeout: "The resolution is still awaiting confirmation",
    idle_timeout: "The sync connection stopped responding",
    transfer_timeout: "The file transfer stopped making progress",
    abnormal_exit: "The sync process stopped before confirming the resolution",
    filesystem: "The workspace file could not be read",
  };
  return labels[code] ? `${labels[code]} (${code})` : code;
}

function failureMessage(error: unknown) {
  if (typeof error === "string") return stableFailureLabel(error);
  if (error instanceof Error) return error.message;
  if (error && typeof error === "object" && "primary" in error) {
    const primary = String(Reflect.get(error, "primary"));
    const cleanup = Reflect.get(error, "cleanup");
    return Array.isArray(cleanup) && cleanup.length > 0
      ? `${stableFailureLabel(primary)}; cleanup=${cleanup.map(String).join(",")}`
      : stableFailureLabel(primary);
  }
  try {
    return JSON.stringify(error) ?? "unknown_error";
  } catch {
    return "unknown_error";
  }
}

export default function ConflictsPane({
  projectId,
  syncRunning,
}: {
  projectId: string;
  syncRunning: boolean;
}) {
  const [conflicts, setConflicts] = useState<ConflictView[]>([]);
  const [loading, setLoading] = useState(syncRunning);
  const [hasLoaded, setHasLoaded] = useState(false);
  const [loadFailure, setLoadFailure] = useState<string | null>(null);
  const [resolving, setResolving] = useState<string | null>(null);
  const [actionFailure, setActionFailure] = useState<string | null>(null);
  const [operationsFailure, setOperationsFailure] = useState<string | null>(null);
  const [operations, setOperations] = useState<ConflictOperation[]>([]);
  const [receipt, setReceipt] = useState<ResolutionReceipt | null>(null);
  const scopeRef = useRef<ConflictRequestScope | null>(null);
  const activeProjectRef = useRef(projectId);
  activeProjectRef.current = projectId;

  async function refresh(scope = scopeRef.current) {
    if (!scope || !scope.beginRefresh()) return;
    setLoading(true);
    try {
      let refreshAgain = false;
      do {
        const [conflictsResult, operationsResult] = await Promise.allSettled([
          syncRunning
            ? invoke<ConflictView[]>("list_sync_conflicts", { projectId })
            : Promise.resolve<ConflictView[] | null>(null),
          invoke<ConflictOperation[]>("list_sync_conflict_operations", {
            projectId,
          }),
        ]);
        if (!scope.isActive() || scopeRef.current !== scope) return;

        if (conflictsResult.status === "fulfilled") {
          if (conflictsResult.value !== null) {
            setConflicts(conflictsResult.value);
            setHasLoaded(true);
            setLoadFailure(null);
          }
        } else {
          setLoadFailure(failureMessage(conflictsResult.reason));
        }

        if (operationsResult.status === "fulfilled") {
          setOperations(operationsResult.value);
          setOperationsFailure(null);
        } else {
          setOperationsFailure(failureMessage(operationsResult.reason));
        }

        refreshAgain = scope.finishRefresh();
        if (refreshAgain && !scope.beginRefresh()) refreshAgain = false;
      } while (refreshAgain);
    } finally {
      scope.finishRefresh();
      if (scope.isActive() && scopeRef.current === scope) setLoading(false);
    }
  }

  useEffect(() => {
    const scope = new ConflictRequestScope();
    scopeRef.current = scope;
    let timer: ReturnType<typeof setTimeout> | undefined;
    setConflicts([]);
    setLoading(syncRunning);
    setHasLoaded(false);
    setLoadFailure(null);
    setResolving(null);
    setActionFailure(null);
    setOperationsFailure(deferredCleanupFailures.get(projectId) ?? null);
    deferredCleanupFailures.delete(projectId);
    setOperations([]);
    setReceipt(null);

    async function poll() {
      await refresh(scope);
      if (scope.isActive() && syncRunning) {
        timer = window.setTimeout(poll, POLL_INTERVAL_MS);
      }
    }

    void poll();
    return () => {
      const cleanup = scope.deactivate();
      if (scopeRef.current === scope) scopeRef.current = null;
      if (timer !== undefined) clearTimeout(timer);
      void invoke("cancel_sync_conflict_generation", {
        projectId,
        projectGeneration: cleanup.projectGeneration,
      }).catch((error) => {
        const message = failureMessage(error);
        if (
          activeProjectRef.current === projectId &&
          scopeRef.current?.isActive()
        ) {
          setOperationsFailure(message);
        } else {
          deferredCleanupFailures.set(projectId, message);
        }
      });
    };
  }, [projectId, syncRunning]);

  async function resolve(conflict: ConflictView, choice: ConflictChoice) {
    const scope = scopeRef.current;
    if (!scope || !conflict.canResolve) return;
    const identity: ConflictControlIdentity | null = scope.beginResolution(
      `${conflict.conflictId}:${conflict.conflictRevision}`,
    );
    if (!identity) return;
    setResolving(conflict.conflictId);
    setActionFailure(null);
    setReceipt(null);
    try {
      const result = await runConflictResolution({
        invokeCommand: invoke,
        projectId,
        identity,
        conflict,
        choice,
        refresh: () => refresh(scope),
      });
      if (!scope.acceptsResolution(identity) || scopeRef.current !== scope) return;
      if (result.ok) {
        setReceipt(result.receipt);
      } else {
        setActionFailure(failureMessage(result.error));
      }
    } finally {
      scope.finishResolution(identity);
      if (scope.isActive() && scopeRef.current === scope) setResolving(null);
    }
  }

  return (
    <ConflictPaneContent
      syncRunning={syncRunning}
      conflicts={conflicts}
      loading={loading}
      hasLoaded={hasLoaded}
      loadFailure={loadFailure}
      actionFailure={actionFailure}
      operationsFailure={operationsFailure}
      operations={operations}
      resolving={resolving}
      receipt={receipt}
      onRefresh={() => void refresh()}
      onResolve={(conflict, choice) => void resolve(conflict, choice)}
    />
  );
}
