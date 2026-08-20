import { useEffect, useMemo, useState, type CSSProperties } from "react";

import { previewClaudeFile } from "../../claude/api.ts";
import {
  flattenVisible,
  listingsFromEntries,
  mergeExplorerTrees,
  sideForExplorerPath,
} from "../../claude/file-tree.ts";
import { projectPassword } from "../../claude/store.ts";
import type { ClaudeFilePreview, ClaudeProject, ClaudeState } from "../../claude/types.ts";

export function FileExplorer({
  project,
  files,
}: {
  project: ClaudeProject;
  files: ClaudeState["filesByProject"][string];
}) {
  const [preview, setPreview] = useState<ClaudeFilePreview | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const tree = useMemo(() => {
    const local = listingsFromEntries(files.filter((file) => file.side !== "remote"));
    const remote = listingsFromEntries(files.filter((file) => file.side === "remote"));
    return mergeExplorerTrees(local, remote);
  }, [files]);
  const visible = flattenVisible(tree, expanded);

  useEffect(() => {
    const dirs = tree.filter((node) => node.kind === "directory").map((node) => node.path);
    setExpanded((current) => {
      if (current.size === 0 && dirs.length > 0) return new Set(dirs);
      return current;
    });
  }, [tree]);

  const openPath = (path: string, kind: "file" | "directory") => {
    if (kind === "directory") {
      setExpanded((current) => {
        const next = new Set(current);
        if (next.has(path)) next.delete(path);
        else next.add(path);
        return next;
      });
      return;
    }
    void previewClaudeFile({
      host: project.host,
      user: project.user,
      port: project.port,
      password: projectPassword(project.id),
      keyPath: project.keyPath,
      hostAlias: project.hostAlias,
      auth: project.auth,
      localRoot: project.localRoot,
      remoteRoot: project.remoteRoot,
      path,
      side: sideForExplorerPath(path, files),
    }).then(setPreview);
  };

  return (
    <div className="lumio-claude-files">
      <div className="lumio-claude-explorer">
        <p className="dim lumio-claude-explorer-root">{project.name}</p>
        {visible.length === 0 ? (
          <p>还没有同步下来的文件。</p>
        ) : (
          visible.map((node) => (
            <button
              className={`lumio-claude-file-row is-${node.kind} is-${node.change}`}
              key={node.path}
              onClick={() => openPath(node.path, node.kind)}
              style={{ "--depth": String(node.depth) } as CSSProperties}
              type="button"
            >
              <span className="chev" aria-hidden="true">
                {node.kind === "directory" ? (expanded.has(node.path) ? "▾" : "▸") : ""}
              </span>
              <span className="name">{node.kind === "directory" ? `${node.name}/` : node.name}</span>
              {node.badge ? <span className={`badge is-${node.badge}`}>{node.badge}</span> : null}
            </button>
          ))
        )}
      </div>
      {preview ? (
        <pre className="lumio-claude-preview">
          {preview.tooLarge
            ? "文件太大，没法在这里预览。"
            : preview.binary
              ? "这是二进制文件，没法预览。"
              : preview.content}
        </pre>
      ) : null}
    </div>
  );
}
