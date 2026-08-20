import { useEffect, useState } from "react";

import { tagColorDiff } from "../../claude/color-diff.ts";
import { resolveProjectConflict } from "../../claude/session.ts";
import type { ClaudeConflictResolution, ClaudeState } from "../../claude/types.ts";

export function ConflictsPane({
  projectId,
  conflicts,
}: {
  projectId: string;
  conflicts: ClaudeState["conflictsByProject"][string];
}) {
  const [selectedId, setSelectedId] = useState<string | null>(conflicts[0]?.id ?? null);
  const current = conflicts.find((item) => item.id === selectedId) ?? conflicts[0] ?? null;
  const tagged = tagColorDiff(current?.localContent ?? "", current?.remoteContent ?? "");

  useEffect(() => {
    if (!conflicts.some((item) => item.id === selectedId)) {
      setSelectedId(conflicts[0]?.id ?? null);
    }
  }, [conflicts, selectedId]);

  const resolve = (resolution: ClaudeConflictResolution) => {
    if (!current) return;
    void resolveProjectConflict(projectId, current.id, resolution);
  };

  if (conflicts.length === 0) {
    return (
      <div className="lumio-claude-files">
        <p>暂无冲突。远端和本机的改动不会被静默覆盖。</p>
      </div>
    );
  }

  return (
    <div className="lumio-claude-conflicts">
      <ul className="lumio-claude-conflict-list">
        {conflicts.map((conflict) => (
          <li key={conflict.id}>
            <button
              className={current?.id === conflict.id ? "is-on" : ""}
              onClick={() => setSelectedId(conflict.id)}
              type="button"
            >
              <strong>{conflict.path}</strong>
              <span className="dim">{conflict.kindLabel}</span>
            </button>
          </li>
        ))}
      </ul>
      {current ? (
        <div className="lumio-claude-conflict-detail">
          <div className="lumio-claude-conflict-actions">
            <button className="lumio-button is-secondary" onClick={() => resolve("keepLocal")} type="button">
              保留本地
            </button>
            <button className="lumio-button is-secondary" onClick={() => resolve("keepRemote")} type="button">
              保留远端
            </button>
            <button className="lumio-button is-secondary" onClick={() => resolve("keepBoth")} type="button">
              两者都保留
            </button>
          </div>
          <div className="lumio-claude-color-diff">
            {tagged.map((line, index) => (
              <div className={`ln is-${line.tag}`} key={`${index}-${line.tag}`}>
                {line.text || " "}
              </div>
            ))}
          </div>
        </div>
      ) : null}
    </div>
  );
}
