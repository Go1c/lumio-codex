export type ConflictChoice = "current" | "incoming" | "merged" | "delete";

export interface ConflictSide {
  path: string | null;
  pathRevision: string;
  contentHash: string | null;
  size: number;
  modifiedAtMs: number;
  executable: boolean;
  tombstone: boolean;
}

export interface PendingResolution {
  operationId: string;
  choice: ConflictChoice;
  contentHash: string | null;
  size: number | null;
}

export interface ConflictView {
  conflictId: string;
  conflictRevision: string;
  path: string;
  kind: "content" | "delete_modify" | "rename" | "binary";
  status:
    | "waiting_blobs"
    | "manual"
    | "auto_ready"
    | "resolving"
    | "refresh_required";
  ancestor: ConflictSide;
  current: ConflictSide;
  incoming: ConflictSide;
  createdByOperationId: string;
  pendingResolution: PendingResolution | null;
  canResolve: boolean;
  blockedReason: string | null;
}

export interface ResolutionReceipt {
  status: "queued";
  operationId: string;
}

export interface ConflictOperation {
  requestId: string;
  projectGeneration: string;
  conflictId: string;
  conflictRevision: string;
  choice: ConflictChoice;
  phase: "pending" | "dispatched" | "queued" | "failed" | "cancelled";
  receipt: ResolutionReceipt | null;
  error: string | null;
}

interface ConflictPaneContentProps {
  syncRunning: boolean;
  conflicts: ConflictView[];
  loading: boolean;
  hasLoaded: boolean;
  loadFailure: string | null;
  actionFailure: string | null;
  operationsFailure: string | null;
  operations: ConflictOperation[];
  resolving: string | null;
  receipt: ResolutionReceipt | null;
  onRefresh: () => void;
  onResolve: (conflict: ConflictView, choice: ConflictChoice) => void;
}

function formatSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function sideState(side: ConflictSide) {
  if (side.tombstone) return "Deleted";
  return `${formatSize(side.size)} at revision ${side.pathRevision}`;
}

function kindLabel(kind: ConflictView["kind"]) {
  switch (kind) {
    case "delete_modify":
      return "Delete and edit";
    case "binary":
      return "Binary content";
    case "rename":
      return "Rename";
    default:
      return "Content";
  }
}

function statusLabel(conflict: ConflictView) {
  if (conflict.pendingResolution) {
    return `Resolving: ${conflict.pendingResolution.choice}`;
  }
  switch (conflict.status) {
    case "waiting_blobs":
      return "Receiving files";
    case "auto_ready":
      return "Preparing resolution";
    case "resolving":
      return "Resolving";
    case "refresh_required":
      return "Refresh required";
    default:
      return "Needs decision";
  }
}

function blockedReasonLabel(reason: string) {
  const labels: Record<string, string> = {
    waiting_blobs: "Required file content is still downloading",
    automatic_resolution_pending: "Automatic resolution is still pending",
    resolution_pending: "A resolution is already pending",
    refresh_required: "The conflict changed and must be refreshed",
    selected_side_deleted: "The selected version has been deleted",
  };
  return `${labels[reason] ?? "Resolution is temporarily unavailable"} (${reason})`;
}

function operationLabel(operation: ConflictOperation) {
  switch (operation.phase) {
    case "pending":
      return "Waiting to send";
    case "dispatched":
      return "Awaiting agent confirmation";
    case "queued":
      return operation.receipt
        ? `Queued as ${operation.receipt.operationId}`
        : "Queued";
    case "cancelled":
      return "Cancelled before sending";
    case "failed":
      return `Failed: ${operation.error ?? "unknown_error"}`;
  }
}

function SideRow({ label, side }: { label: string; side: ConflictSide }) {
  const path = side.path ?? "Deleted path";
  const hash = side.contentHash ?? "No content";
  return (
    <div className="conflict-side-row">
      <strong>{label}</strong>
      <span>{sideState(side)}</span>
      <span className="conflict-side-value" title={path}>
        Path: <code>{path}</code>
      </span>
      <span className="conflict-side-value" title={hash}>
        Hash: <code>{hash}</code>
      </span>
    </div>
  );
}

export function ConflictPaneContent({
  syncRunning,
  conflicts,
  loading,
  hasLoaded,
  loadFailure,
  actionFailure,
  operationsFailure,
  operations,
  resolving,
  receipt,
  onRefresh,
  onResolve,
}: ConflictPaneContentProps) {
  const listUnavailable = conflicts.length === 0 && loadFailure !== null;
  const initialLoading =
    syncRunning && conflicts.length === 0 && (!hasLoaded || loading);
  const empty =
    syncRunning &&
    conflicts.length === 0 &&
    hasLoaded &&
    !loading &&
    loadFailure === null;
  const countLabel = !syncRunning
    ? "Sync stopped"
    : loadFailure
    ? hasLoaded
      ? `${conflicts.length} unresolved; update failed`
      : "Unavailable"
    : hasLoaded
      ? `${conflicts.length} unresolved`
      : "Loading";
  const failures = [
    { key: "resolution", label: "Resolution", message: actionFailure },
    { key: "list", label: "Conflict list", message: loadFailure },
    { key: "operations", label: "Decision history", message: operationsFailure },
  ].filter(
    (failure): failure is { key: string; label: string; message: string } =>
      failure.message !== null,
  );

  return (
    <section className="conflicts-pane" aria-label="Sync conflicts">
      <div className="conflicts-toolbar">
        <div>
          <strong>Conflicts</strong>
          <span>{countLabel}</span>
        </div>
        <button
          className="btn btn-secondary conflict-refresh"
          type="button"
          onClick={onRefresh}
          disabled={loading || resolving !== null}
        >
          {loading ? "Refreshing..." : "Refresh"}
        </button>
      </div>

      {failures.length > 0 && (
        <div className="conflict-error" role="alert">
          <strong>Conflict operation failed</strong>
          <div>
            {failures.map((failure) => (
              <span key={failure.key}>
                <b>{failure.label}</b>
                <code>{failure.message}</code>
              </span>
            ))}
          </div>
        </div>
      )}
      {receipt && (
        <div className="conflict-receipt" role="status">
          Resolution queued as <code>{receipt.operationId}</code>
        </div>
      )}

      {operations.length > 0 && (
        <div className="conflict-operations" aria-label="Recent conflict decisions">
          <strong>Recent decisions</strong>
          {operations.slice(0, 5).map((operation) => (
            <div
              className={`conflict-operation conflict-operation-${operation.phase}`}
              key={operation.requestId}
            >
              <span>
                {operation.choice} at revision {operation.conflictRevision}
              </span>
              <code>{operationLabel(operation)}</code>
            </div>
          ))}
        </div>
      )}

      {!syncRunning && (
        <div className="conflicts-empty" role="status">
          <strong>Conflicts unavailable</strong>
          <span>Start sync to load and resolve current conflicts.</span>
        </div>
      )}

      {initialLoading && !listUnavailable && (
        <div className="conflicts-empty" role="status">
          <strong>Loading conflicts</strong>
          <span>Waiting for the current workspace state.</span>
        </div>
      )}
      {listUnavailable && (
        <div className="conflicts-empty" role="status">
          <strong>Conflict status unavailable</strong>
          <span>The current conflict list could not be verified.</span>
        </div>
      )}
      {empty && (
        <div className="conflicts-empty" role="status">
          <strong>No unresolved conflicts</strong>
          <span>Local and remote changes agree.</span>
        </div>
      )}
      {conflicts.length > 0 && (
        <div className="conflict-list">
          {conflicts.map((conflict) => {
            const busy = resolving === conflict.conflictId;
            const disabled = resolving !== null || !conflict.canResolve;
            return (
              <article className="conflict-item" key={conflict.conflictId}>
                <header>
                  <div>
                    <strong>{conflict.path}</strong>
                    <span>{kindLabel(conflict.kind)} resolution target</span>
                  </div>
                  <span className={`conflict-status conflict-status-${conflict.status}`}>
                    {busy ? "Queuing..." : statusLabel(conflict)}
                  </span>
                </header>

                <div className="conflict-sides">
                  <SideRow label="Base" side={conflict.ancestor} />
                  <SideRow label="Local" side={conflict.current} />
                  <SideRow label="Remote" side={conflict.incoming} />
                </div>

                {conflict.blockedReason && (
                  <p className="conflict-blocked">
                    {blockedReasonLabel(conflict.blockedReason)}
                  </p>
                )}
                <div className="conflict-actions">
                  <button
                    className="btn btn-secondary"
                    type="button"
                    disabled={disabled || conflict.current.tombstone}
                    onClick={() => onResolve(conflict, "current")}
                  >
                    Keep local
                  </button>
                  <button
                    className="btn btn-secondary"
                    type="button"
                    disabled={disabled || conflict.incoming.tombstone}
                    onClick={() => onResolve(conflict, "incoming")}
                  >
                    Use remote
                  </button>
                  <button
                    className="btn btn-secondary"
                    type="button"
                    title={`Resolve with the edited file at ${conflict.path}`}
                    disabled={disabled}
                    onClick={() => onResolve(conflict, "merged")}
                  >
                    Use edited target
                  </button>
                  <button
                    className="btn btn-danger"
                    type="button"
                    disabled={disabled}
                    onClick={() => onResolve(conflict, "delete")}
                  >
                    Delete
                  </button>
                </div>
              </article>
            );
          })}
        </div>
      )}
    </section>
  );
}
