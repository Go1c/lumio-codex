import { useEffect, useMemo, useState } from "react";
import { t } from "../i18n";
import { toApiError } from "../lib/api";
import { formatRelative } from "../lib/format";
import { useApi } from "../state/ApiProvider";
import { useToast } from "../state/ToastProvider";
import type { Conflict, Resolution } from "../lib/types";

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
    setBusy(true);
    try {
      const receipt = await api.resolveConflict(projectId, conflict.id, resolution);
      await onChanged();
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
      toast(toApiError(caught).message);
    } finally {
      setBusy(false);
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
                disabled={busy}
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
