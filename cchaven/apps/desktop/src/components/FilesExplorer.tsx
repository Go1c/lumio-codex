import { useCallback, useEffect, useMemo, useState } from "react";
import { t } from "../i18n";
import { toApiError } from "../lib/api";
import { extensionOf, formatBytes, formatRelative } from "../lib/format";
import { useApi } from "../state/ApiProvider";
import { useToast } from "../state/ToastProvider";
import { ContextMenu, type MenuItem } from "./ui";
import type { FileNode, FilePreview } from "../lib/types";

/** Per-file sync badge; 6.3 colours, nothing invented. */
type FileSync = "synced" | "syncing" | "conflict";

const SYNC_META: Record<FileSync, { icon: string; label: string; color: string }> = {
  synced: { icon: "✓", label: t("sync.label.synced"), color: "var(--green)" },
  syncing: { icon: "↑↓", label: t("sync.label.syncing"), color: "var(--blue)" },
  conflict: { icon: "⚠", label: t("sync.label.conflicts"), color: "var(--orange)" },
};

/** Files touched in the last minute are still on their way to the server. */
const SYNCING_WINDOW_MS = 60_000;

type Editing =
  | { mode: "newFile" | "newFolder"; parent: string }
  | { mode: "rename"; path: string }
  | null;

export interface FilesExplorerProps {
  projectId: string;
  projectName: string;
  conflictPaths: string[];
  onGoToConflicts: () => void;
}

export function FilesExplorer({
  projectId,
  projectName,
  conflictPaths,
  onGoToConflicts,
}: FilesExplorerProps) {
  const api = useApi();
  const { toast } = useToast();

  const [tree, setTree] = useState<FileNode[]>([]);
  const [recent, setRecent] = useState<FileNode[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [expanded, setExpanded] = useState<Set<string>>(new Set(["src"]));
  const [rootOpen, setRootOpen] = useState(true);
  const [selected, setSelected] = useState<string | null>(null);
  const [tabs, setTabs] = useState<string[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [preview, setPreview] = useState<FilePreview | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; node: FileNode | null } | null>(null);
  const [editing, setEditing] = useState<Editing>(null);
  const [draft, setDraft] = useState("");

  const conflicts = useMemo(() => new Set(conflictPaths), [conflictPaths]);

  const load = useCallback(async () => {
    try {
      const [nodes, recentNodes] = await Promise.all([
        api.listFiles(projectId),
        api.recentFiles(projectId, 6),
      ]);
      setTree(nodes);
      setRecent(recentNodes);
      setError("");
    } catch (caught) {
      setError(toApiError(caught).message);
    } finally {
      setLoading(false);
    }
  }, [api, projectId]);

  useEffect(() => {
    setLoading(true);
    void load();
  }, [load]);

  function syncOf(node: FileNode): FileSync {
    if (conflicts.has(node.path)) return "conflict";
    if (node.modifiedMs && Date.now() - node.modifiedMs < SYNCING_WINDOW_MS) return "syncing";
    return "synced";
  }

  async function openFile(path: string) {
    setSelected(path);
    setTabs((current) => (current.includes(path) ? current : [...current, path]));
    setActive(path);
    try {
      setPreview(await api.readFile(projectId, path));
    } catch (caught) {
      setPreview(null);
      setError(toApiError(caught).message);
    }
  }

  function closeTab(path: string) {
    const rest = tabs.filter((tab) => tab !== path);
    setTabs(rest);
    if (active === path) {
      const next = rest[rest.length - 1] ?? null;
      setActive(next);
      if (next) void openFile(next);
      else setPreview(null);
    }
  }

  function toggleDirectory(path: string) {
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  function startCreate(mode: "newFile" | "newFolder", parent: string) {
    setDraft("");
    setEditing({ mode, parent });
    if (parent) setExpanded((current) => new Set(current).add(parent));
    setMenu(null);
  }

  async function commitCreate() {
    if (!editing || editing.mode === "rename") return;
    const name = draft.trim();
    if (!name) {
      setEditing(null);
      return;
    }
    try {
      const path = await api.createEntry(
        projectId,
        editing.parent,
        name,
        editing.mode === "newFolder" ? "directory" : "file",
      );
      setEditing(null);
      await load();
      toast(t("workspace.createdSyncing", { name }));
      if (editing.mode === "newFile") void openFile(path);
    } catch (caught) {
      setEditing(null);
      toast(toApiError(caught).message);
    }
  }

  async function commitRename() {
    if (!editing || editing.mode !== "rename") return;
    const name = draft.trim();
    const previousPath = editing.path;
    setEditing(null);
    if (!name) return;
    try {
      const next = await api.renameEntry(projectId, previousPath, name);
      await load();
      setTabs((current) => current.map((tab) => (tab === previousPath ? next : tab)));
      setActive((current) => (current === previousPath ? next : current));
      toast(t("workspace.renamedSyncing", { name }));
    } catch (caught) {
      toast(toApiError(caught).message);
    }
  }

  async function remove(node: FileNode) {
    setMenu(null);
    try {
      const ticket = await api.deleteEntry(projectId, node.path);
      if (tabs.includes(node.path)) closeTab(node.path);
      await load();
      // 10 秒撤销窗口（6.4）；过期后清掉暂存副本。
      toast(t("workspace.deletedBothSides", { name: node.name }), {
        action: {
          label: t("common.undo"),
          onClick: () => {
            void api
              .undoDelete(projectId, ticket.token)
              .then(load)
              .then(() => toast(t("workspace.deleteUndone", { name: node.name })))
              .catch((caught) => toast(toApiError(caught).message));
          },
        },
        onExpire: () => void api.purgeDelete(ticket.token),
      });
    } catch (caught) {
      toast(toApiError(caught).message);
    }
  }

  async function refresh() {
    await load();
    toast(t("workspace.refreshed"));
  }

  function copyPath(path: string) {
    void navigator.clipboard?.writeText(path).catch(() => undefined);
    toast(t("common.copied"));
  }

  if (loading) {
    return (
      <div className="files">
        <div className="file-tree" style={{ padding: 12 }} data-testid="files-skeleton">
          {[80, 120, 100, 90, 110].map((width, index) => (
            <div
              key={index}
              className="skeleton"
              style={{ height: 16, width, margin: "10px 6px" }}
            />
          ))}
        </div>
        <div className="editor" />
      </div>
    );
  }

  if (error && tree.length === 0) {
    return (
      <div className="files" style={{ alignItems: "center", justifyContent: "center" }}>
        <div style={{ textAlign: "center" }}>
          <p style={{ color: "var(--red)", marginBottom: 12 }}>{t("workspace.filesError")}</p>
          <button type="button" className="btn btn-secondary" onClick={() => void load()}>
            {t("common.retry")}
          </button>
        </div>
      </div>
    );
  }

  const inlineInput = (commit: () => void) => (
    <input
      className="inline-edit"
      aria-label={editing?.mode === "rename" ? t("workspace.rename") : t("workspace.newFile")}
      value={draft}
      autoFocus
      placeholder={
        editing?.mode === "newFolder"
          ? t("workspace.folderNamePlaceholder")
          : t("workspace.fileNamePlaceholder")
      }
      onChange={(event) => setDraft(event.target.value)}
      onKeyDown={(event) => {
        if (event.key === "Enter") commit();
        if (event.key === "Escape") setEditing(null);
      }}
      onClick={(event) => event.stopPropagation()}
    />
  );

  function renderNodes(nodes: FileNode[], depth: number) {
    return nodes.map((node) => {
      const renaming = editing?.mode === "rename" && editing.path === node.path;
      const open = expanded.has(node.path);
      const sync = SYNC_META[syncOf(node)];
      return (
        <div key={node.path}>
          <button
            type="button"
            className={`file-row ${selected === node.path ? "sel" : ""} ${
              active === node.path ? "opened" : ""
            }`}
            style={{ paddingLeft: 8 + depth * 14 }}
            onClick={() => {
              setSelected(node.path);
              if (node.kind === "directory") toggleDirectory(node.path);
              else void openFile(node.path);
            }}
            onContextMenu={(event) => {
              event.preventDefault();
              event.stopPropagation();
              setSelected(node.path);
              setMenu({ x: event.clientX, y: event.clientY, node });
            }}
          >
            {Array.from({ length: depth }).map((_, index) => (
              <span key={index} className="guide" />
            ))}
            <span className="chev" aria-hidden="true">
              {node.kind === "directory" ? (open ? "▾" : "▸") : ""}
            </span>
            <span className="fic" aria-hidden="true">
              {fileIcon(node, open)}
            </span>
            {renaming ? inlineInput(commitRename) : <span className="fname">{node.name}</span>}
            {!renaming && node.kind === "file" && (
              <span className="sync-ic" title={sync.label} style={{ color: sync.color }}>
                {sync.icon}
              </span>
            )}
          </button>

          {node.kind === "directory" && open && (
            <>
              {editing && editing.mode !== "rename" && editing.parent === node.path && (
                <div className="file-row" style={{ paddingLeft: 8 + (depth + 1) * 14 }}>
                  <span className="chev" />
                  <span className="fic" aria-hidden="true">
                    {editing.mode === "newFolder" ? "📁" : "📄"}
                  </span>
                  {inlineInput(commitCreate)}
                </div>
              )}
              {node.children && renderNodes(node.children, depth + 1)}
            </>
          )}
        </div>
      );
    });
  }

  const activeNode = active ? findNode(tree, active) : null;
  const menuItems: MenuItem[] = buildMenuItems();

  function buildMenuItems(): MenuItem[] {
    if (!menu) return [];
    const node = menu.node;
    if (!node) {
      return [
        { label: t("workspace.newFile"), onSelect: () => startCreate("newFile", "") },
        { label: t("workspace.newFolder"), onSelect: () => startCreate("newFolder", "") },
        { label: t("common.refresh"), onSelect: () => void refresh(), separatorBefore: true },
      ];
    }
    const items: MenuItem[] = [];
    if (node.kind === "file") {
      items.push({ label: t("workspace.open"), onSelect: () => void openFile(node.path) });
    } else {
      items.push(
        { label: t("workspace.newFile"), onSelect: () => startCreate("newFile", node.path) },
        { label: t("workspace.newFolder"), onSelect: () => startCreate("newFolder", node.path) },
      );
    }
    items.push({
      label: t("workspace.rename"),
      separatorBefore: true,
      onSelect: () => {
        setDraft(node.name);
        setEditing({ mode: "rename", path: node.path });
        setMenu(null);
      },
    });
    if (node.kind === "file") {
      items.push({ label: t("common.copyPath"), onSelect: () => copyPath(node.path) });
    }
    items.push({
      label: t("common.revealInFinder"),
      onSelect: () => void api.revealEntry(projectId, node.path),
    });
    items.push({
      label: t("common.delete"),
      danger: true,
      separatorBefore: true,
      onSelect: () => void remove(node),
    });
    return items;
  }

  return (
    <div className="files">
      <div
        className="file-tree"
        data-testid="file-tree"
        onContextMenu={(event) => {
          event.preventDefault();
          setMenu({ x: event.clientX, y: event.clientY, node: null });
        }}
      >
        <div className="explorer-head">{t("workspace.explorer")}</div>
        <div className="root-row" onClick={() => setRootOpen((open) => !open)}>
          <span className="chev" aria-hidden="true">
            {rootOpen ? "▾" : "▸"}
          </span>
          <strong>{projectName.toUpperCase()}</strong>
          <span className="tools" onClick={(event) => event.stopPropagation()}>
            <button
              type="button"
              className="icon-btn"
              aria-label={t("workspace.newFile")}
              title={t("workspace.newFile")}
              onClick={() => startCreate("newFile", selectedFolder(tree, selected))}
            >
              ＋📄
            </button>
            <button
              type="button"
              className="icon-btn"
              aria-label={t("workspace.newFolder")}
              title={t("workspace.newFolder")}
              onClick={() => startCreate("newFolder", selectedFolder(tree, selected))}
            >
              ＋📁
            </button>
            <button
              type="button"
              className="icon-btn"
              aria-label={t("common.refresh")}
              title={t("common.refresh")}
              onClick={() => void refresh()}
            >
              ⟳
            </button>
            <button
              type="button"
              className="icon-btn"
              aria-label={t("workspace.collapseAll")}
              title={t("workspace.collapseAll")}
              onClick={() => setExpanded(new Set())}
            >
              ⊟
            </button>
          </span>
        </div>

        {rootOpen && (
          <>
            {editing && editing.mode !== "rename" && editing.parent === "" && (
              <div className="file-row" style={{ paddingLeft: 8 }}>
                <span className="chev" />
                <span className="fic" aria-hidden="true">
                  {editing.mode === "newFolder" ? "📁" : "📄"}
                </span>
                {inlineInput(commitCreate)}
              </div>
            )}
            {tree.length === 0 && !editing ? (
              <div style={{ padding: "18px 14px", color: "var(--gray)", fontSize: 13 }}>
                {t("workspace.filesEmpty")}
                <div style={{ marginTop: 6, fontSize: 12 }}>{t("workspace.filesEmptyHint")}</div>
              </div>
            ) : (
              renderNodes(tree, 0)
            )}
          </>
        )}
      </div>

      <div className="editor">
        {tabs.length > 0 && (
          <div className="ed-tabs" role="tablist">
            {tabs.map((tab) => {
              const node = findNode(tree, tab);
              const name = node?.name ?? tab.split("/").pop() ?? tab;
              return (
                <div
                  key={tab}
                  role="tab"
                  aria-selected={active === tab}
                  className={`ed-tab ${active === tab ? "active" : ""}`}
                  onClick={() => void openFile(tab)}
                >
                  <span aria-hidden="true">{node ? fileIcon(node, false) : "📄"}</span>
                  <span>{name}</span>
                  {conflicts.has(tab) && (
                    <span style={{ color: "var(--orange)", fontSize: 10 }} title={t("sync.label.conflicts")}>
                      ⚠
                    </span>
                  )}
                  <button
                    type="button"
                    className="x"
                    aria-label={`${t("common.close")} ${name}`}
                    onClick={(event) => {
                      event.stopPropagation();
                      closeTab(tab);
                    }}
                  >
                    ×
                  </button>
                </div>
              );
            })}
          </div>
        )}

        {active && preview ? (
          <div className="ed-body">
            <div className="crumbs">
              <button type="button" className="crumb" onClick={() => setActive(null)}>
                {projectName}
              </button>
              {active.split("/").map((segment, index, all) => (
                <span key={`${segment}-${index}`}>
                  <span className="crumb-sep">›</span>
                  <span className={index === all.length - 1 ? "crumb last" : "crumb"}>
                    {segment}
                  </span>
                </span>
              ))}
              <span className="crumb-actions">
                <button
                  type="button"
                  className="btn btn-ghost btn-sm"
                  onClick={() => copyPath(active)}
                >
                  {t("common.copyPath")}
                </button>
                <button
                  type="button"
                  className="btn btn-secondary btn-sm"
                  onClick={() => void api.openEntry(projectId, active)}
                >
                  {t("common.openWithEditor")}
                </button>
              </span>
            </div>

            <div className="file-meta">
              {formatBytes(preview.size)} ·{" "}
              {t("workspace.modifiedAt", { when: formatRelative(preview.modifiedMs) })} ·{" "}
              <span style={{ color: SYNC_META[syncOf(activeNode ?? fallbackNode(active))].color }}>
                {SYNC_META[syncOf(activeNode ?? fallbackNode(active))].label}
              </span>
              {conflicts.has(active) && (
                <button type="button" className="btn btn-ghost btn-sm" onClick={onGoToConflicts}>
                  {t("workspace.goToConflicts")}
                </button>
              )}
            </div>

            {preview.tooLarge ? (
              <p style={{ color: "var(--gray)" }}>{t("workspace.tooLarge")}</p>
            ) : preview.binary ? (
              <p style={{ color: "var(--gray)" }}>{t("workspace.binaryFile")}</p>
            ) : (
              <div className="code">
                {preview.content.split("\n").map((line, index) => (
                  <div className="code-line" key={index}>
                    <span className="ln-no">{index + 1}</span>
                    <span className="ln-body">{line || " "}</span>
                  </div>
                ))}
              </div>
            )}
          </div>
        ) : (
          <div className="ed-body">
            <div className="recent-panel">
              <h4>{t("workspace.recentTitle")}</h4>
              <p className="lead">{t("workspace.recentBody")}</p>
              {recent.length === 0 && (
                <p style={{ color: "var(--gray)", fontSize: 13 }}>{t("workspace.filesEmpty")}</p>
              )}
              {recent.map((node) => {
                const sync = SYNC_META[syncOf(node)];
                return (
                  <button
                    type="button"
                    key={node.path}
                    className="recent-row"
                    onClick={() => void openFile(node.path)}
                  >
                    <span aria-hidden="true">{fileIcon(node, false)}</span>
                    <span className="rp">{node.path}</span>
                    <span className="rw">{formatRelative(node.modifiedMs)}</span>
                    <span className="sync-ic" style={{ color: sync.color }} title={sync.label}>
                      {sync.icon}
                    </span>
                  </button>
                );
              })}
            </div>
          </div>
        )}
      </div>

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={menuItems}
          label={menu.node ? menu.node.name : t("workspace.explorer")}
          onClose={() => setMenu(null)}
        />
      )}
    </div>
  );
}

function fallbackNode(path: string): FileNode {
  return { name: path, path, kind: "file" };
}

function findNode(nodes: FileNode[], path: string): FileNode | null {
  for (const node of nodes) {
    if (node.path === path) return node;
    if (node.children) {
      const found = findNode(node.children, path);
      if (found) return found;
    }
  }
  return null;
}

/** New entries land in the selected folder, or its parent when a file is selected. */
function selectedFolder(tree: FileNode[], selected: string | null): string {
  if (!selected) return "";
  const node = findNode(tree, selected);
  if (node?.kind === "directory") return node.path;
  return selected.includes("/") ? selected.slice(0, selected.lastIndexOf("/")) : "";
}

function fileIcon(node: FileNode, open: boolean): string {
  if (node.kind === "directory") return open ? "📂" : "📁";
  switch (extensionOf(node.name)) {
    case "rs":
      return "🦀";
    case "md":
      return "📝";
    case "toml":
    case "json":
    case "yaml":
    case "yml":
      return "⚙️";
    default:
      return node.name.startsWith(".") ? "🔧" : "📄";
  }
}
