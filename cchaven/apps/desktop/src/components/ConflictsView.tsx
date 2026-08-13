import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { t } from "../i18n";
import { toApiError } from "../lib/api";
import { formatRelative } from "../lib/format";
import { useApi } from "../state/ApiProvider";
import { useToast } from "../state/ToastProvider";
import { ConflictRequestScope } from "./ConflictRequestScope";
import type {
  Conflict,
  ConflictControlIdentity,
  ConflictResolutionOperationView,
  Resolution,
} from "../lib/types";

const RESOLUTIONS: Array<[Resolution, string]> = [
  ["keepLocal", t("workspace.keepLocal")],
  ["keepRemote", t("workspace.keepRemote")],
  ["keepBoth", t("workspace.keepBoth")],
];

/** 5.5「冲突」 — list, side-by-side diff, three resolutions, 10 秒可撤销。 */
export function ConflictsView({
  projectId,
  conflicts,
  onChanged,
}: {
  projectId: string;
  conflicts: Conflict[];
  onChanged: () => void | Promise<void>;
}) {
  const api = useApi();
  const { toast } = useToast();
  const [selectedId, setSelectedId] = useState<string | null>(conflicts[0]?.id ?? null);
  const [busy, setBusy] = useState(false);
  const [inFlight, setInFlight] = useState<ConflictControlIdentity | null>(null);
  const [operations, setOperations] = useState<ConflictResolutionOperationView[]>([]);
  const [failure, setFailure] = useState("");

  // One scope per mount. It stamps every request with a generation so a reply
  // that arrives after the user has left cannot be applied to a fresh page.
  const scopeRef = useRef<ConflictRequestScope | null>(null);
  if (scopeRef.current === null) scopeRef.current = new ConflictRequestScope();
  const activeProjectRef = useRef(projectId);
  activeProjectRef.current = projectId;

  const loadOperations = useCallback(async () => {
    try {
      const next = await api.listConflictOperations(projectId);
      if (activeProjectRef.current === projectId) setOperations(next);
    } catch {
      // The decision history is a convenience; its absence must not mask the
      // conflicts themselves.
    }
  }, [api, projectId]);

  useEffect(() => {
    void loadOperations();
  }, [loadOperations]);

  // Abandon anything still in flight when the page goes away, so the engine
  // does not hold a request nobody is waiting for.
  useEffect(() => {
    const scope = scopeRef.current;
    return () => {
      if (!scope) return;
      const cleanup = scope.deactivate();
      if (cleanup.activeRequestIds.length === 0) return;
      void api
        .cancelConflictGeneration(projectId, cleanup.projectGeneration)
        .catch(() => undefined);
    };
  }, [api, projectId]);

  useEffect(() => {
    if (!conflicts.some((conflict) => conflict.id === selectedId)) {
      setSelectedId(conflicts[0]?.id ?? null);
    }
  }, [conflicts, selectedId]);

  const current = useMemo(
    () => conflicts.find((conflict) => conflict.id === selectedId) ?? conflicts[0] ?? null,
    [conflicts, selectedId],
  );

  async function resolve(conflict: Conflict, resolution: Resolution) {
    const scope = scopeRef.current;
    const identity = scope?.beginResolution(conflict.id) ?? null;
    if (!identity) return;

    setBusy(true);
    setFailure("");
    setInFlight(identity);
    try {
      const receipt = await api.resolveConflict(
        projectId,
        conflict.id,
        resolution,
        identity,
      );
      // A late reply for a page the user has already left must be dropped.
      if (!scope?.acceptsResolution(identity)) return;
      await onChanged();
      await loadOperations();
      toast(t("workspace.resolved", { path: receipt.path, how: receipt.label }), {
        action: {
          label: t("common.undo"),
          onClick: () => {
            void api
              .undoConflict(projectId, conflict.id)
              .then(onChanged)
              .then(() => toast(t("workspace.resolveUndone", { path: receipt.path })))
              .catch((caught) => toast(toApiError(caught).message));
          },
        },
        onExpire: () => void api.forgetConflictUndo(projectId, conflict.id),
      });
    } catch (caught) {
      if (!scope?.acceptsResolution(identity)) return;
      setFailure(t("conflicts.operationFailed", { detail: toApiError(caught).message }));
    } finally {
      scope?.finishResolution(identity);
      setInFlight(null);
      setBusy(false);
    }
  }

  /** Withdraw a resolution the user no longer wants to wait for. */
  async function cancelInFlight() {
    if (!inFlight) return;
    try {
      await api.cancelConflictRequest(projectId, inFlight);
      await loadOperations();
    } catch (caught) {
      setFailure(t("conflicts.cancelFailed", { detail: toApiError(caught).message }));
    }
  }

  async function resolveAll(resolution: Resolution) {
    for (const conflict of [...conflicts]) {
      // Sequential on purpose: each resolution rewrites the same record file.
      // eslint-disable-next-line no-await-in-loop
      await resolve(conflict, resolution);
    }
  }

  if (conflicts.length === 0) {
    return (
      <div className="empty-state">
        <div className="art" aria-hidden="true">
          ✓
        </div>
        <h3>{t("workspace.conflictsEmptyTitle")}</h3>
        <p>{t("workspace.conflictsEmptyBody")}</p>
      </div>
    );
  }

  return (
    <div className="conflicts">
      <div className="conf-list" role="list">
        {conflicts.map((conflict) => (
          <button
            type="button"
            role="listitem"
            key={conflict.id}
            className={`conf-item ${current?.id === conflict.id ? "sel" : ""}`}
            onClick={() => setSelectedId(conflict.id)}
          >
            <div className="f">⚠ {conflict.path}</div>
            <div className="meta">
              {conflict.kindLabel} · {formatRelative(conflict.detectedAtMs)}
            </div>
          </button>
        ))}
      </div>

      {current && (
        <div className="conf-detail">
          <div className="conf-actions">
            <span className="path">{current.path}</span>
            {RESOLUTIONS.map(([resolution, label]) => (
              <button
                key={resolution}
                type="button"
                className={resolution === "keepBoth" ? "btn btn-ghost" : "btn btn-secondary"}
                disabled={busy || current.canResolve === false}
                onClick={() => void resolve(current, resolution)}
              >
                {label}
              </button>
            ))}
            <select
              aria-label={t("workspace.resolveAll")}
              value=""
              disabled={busy}
              onChange={(event) => {
                const resolution = event.target.value as Resolution | "";
                if (resolution) void resolveAll(resolution);
              }}
            >
              <option value="">{t("workspace.resolveAll")}</option>
              {RESOLUTIONS.map(([resolution, label]) => (
                <option key={resolution} value={resolution}>
                  {label}
                </option>
              ))}
            </select>
          </div>

          {(current.pendingResolution ||
            current.canResolve === false ||
            inFlight ||
            failure) && (
            <div className="conf-state" role="status">
              {current.pendingResolution && (
                <span>
                  {t("conflicts.queuedAs", { how: current.pendingResolution })}
                </span>
              )}
              {current.canResolve === false && !current.pendingResolution && (
                <span>{t("conflicts.blocked")}</span>
              )}
              {inFlight && (
                <>
                  <span>{t("conflicts.inFlight")}</span>
                  <button
                    type="button"
                    className="btn btn-ghost btn-sm"
                    onClick={() => void cancelInFlight()}
                  >
                    {t("conflicts.cancelRequest")}
                  </button>
                </>
              )}
              {failure && <span className="conf-failure">{failure}</span>}
            </div>
          )}

          {operations.length > 0 && (
            <details className="conf-history">
              <summary>{t("conflicts.recentDecisions")}</summary>
              <ul>
                {operations.map((operation) => (
                  <li key={operation.requestId}>
                    {t(`conflicts.phase.${operation.phase}`)}
                    {" · "}
                    {operation.choice}
                    {operation.receipt?.operationId
                      ? ` · ${operation.receipt.operationId}`
                      : ""}
                    {operation.error ? ` · ${operation.error}` : ""}
                  </li>
                ))}
              </ul>
            </details>
          )}

          <div className="diff">
            <DiffPane
              title={t("workspace.localPane", { when: formatRelative(current.local.modifiedMs) })}
              lines={current.local.content.split("\n")}
              other={current.remote.content.split("\n")}
              tone="add"
            />
            <DiffPane
              title={t("workspace.remotePane", {
                when: formatRelative(current.remote.modifiedMs),
              })}
              lines={
                current.remote.deleted
                  ? [t("workspace.remoteDeleted")]
                  : current.remote.content.split("\n")
              }
              other={current.local.content.split("\n")}
              tone="del"
            />
          </div>
        </div>
      )}
    </div>
  );
}

/** Highlights lines the other side does not have; enough to spot the divergence. */
function DiffPane({
  title,
  lines,
  other,
  tone,
}: {
  title: string;
  lines: string[];
  other: string[];
  tone: "add" | "del";
}) {
  const otherSet = useMemo(() => new Set(other), [other]);
  return (
    <div className="pane">
      <h4>{title}</h4>
      {lines.map((line, index) => (
        <div
          key={`${index}-${line}`}
          className={`ln ${line.trim() && !otherSet.has(line) ? tone : ""}`}
        >
          {line || " "}
        </div>
      ))}
    </div>
  );
}
