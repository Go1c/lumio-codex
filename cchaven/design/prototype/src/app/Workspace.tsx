import { useEffect, useMemo, useState } from "react";
import { Project, useDemo } from "../demo";

type Tab = "terminal" | "files" | "conflicts";

interface Conflict {
  id: string;
  file: string;
  kind: string;
  when: string;
}

const INITIAL_CONFLICTS: Conflict[] = [
  { id: "c1", file: "src/engine.rs", kind: "本地与远端同时修改", when: "2 分钟前" },
  { id: "c2", file: "Cargo.toml", kind: "远端已删除，本地已修改", when: "14 分钟前" },
];

export default function Workspace({ project }: { project: Project }) {
  const { showErrors, toast } = useDemo();
  const [tab, setTab] = useState<Tab>("terminal");
  const [conflicts, setConflicts] = useState<Conflict[]>(INITIAL_CONFLICTS);

  return (
    <>
      <div className="ws-head">
        <div className="ws-title">
          <strong>{project.name}</strong>
          <span className="host-chip" title="连接的服务器">🖥 {project.host}</span>
        </div>
        <div className="ws-head-actions">
          {showErrors ? (
            <>
              <span className="conn-pill offline"><span className="dot offline" /> 连接已断开</span>
              <button className="btn btn-secondary" style={{ padding: "4px 12px", fontSize: 12.5 }}>重新连接</button>
            </>
          ) : (
            <span className="conn-pill ok"><span className="dot synced" /> 已连接 · 已全部同步</span>
          )}
          <button
            className="btn btn-ghost"
            style={{ padding: "4px 10px", fontSize: 12.5 }}
            onClick={() => toast(`正在 Finder 中打开 ${project.localRoot}…`)}
          >
            打开本地文件夹
          </button>
        </div>
      </div>

      <div className="ws-tabs">
        {(
          [
            ["terminal", "终端"],
            ["files", "文件"],
            ["conflicts", "冲突"],
          ] as [Tab, string][]
        ).map(([k, label]) => (
          <button key={k} className={`ws-tab ${tab === k ? "active" : ""}`} onClick={() => setTab(k)}>
            {label}
            {k === "conflicts" && conflicts.length > 0 && <span className="badge">{conflicts.length}</span>}
          </button>
        ))}
      </div>
      {tab === "terminal" && <TerminalMock project={project} />}
      {tab === "files" && <FilesMock project={project} />}
      {tab === "conflicts" && <ConflictsMock conflicts={conflicts} setConflicts={setConflicts} />}
    </>
  );
}

/* ---------- 终端 ---------- */

function TerminalMock({ project }: { project: Project }) {
  const { showErrors } = useDemo();
  const [connecting, setConnecting] = useState(true);
  const [retryIn, setRetryIn] = useState(5);

  useEffect(() => {
    const t = setTimeout(() => setConnecting(false), 1200);
    return () => clearTimeout(t);
  }, []);

  useEffect(() => {
    if (!showErrors) return;
    const t = setInterval(() => setRetryIn((r) => (r > 1 ? r - 1 : 5)), 1000);
    return () => clearInterval(t);
  }, [showErrors]);

  if (connecting) {
    return (
      <div className="term" style={{ display: "flex", alignItems: "center", justifyContent: "center" }}>
        <span style={{ color: "#8b949e" }}>
          <span className="spinner" style={{ borderTopColor: "#8b949e" }} /> 正在连接 {project.host}…
        </span>
      </div>
    );
  }

  return (
    <div className="term">
      {showErrors && (
        <div className="term-banner">
          连接已断开，{retryIn} 秒后自动重连…
          <button>立即重连</button>
        </div>
      )}
      <div className="dim">已接入远端会话 cc-{project.name} — 就算合上电脑，对话也会继续保留</div>
      <div>&nbsp;</div>
      <div><span className="p">{project.host}</span>:{project.remoteRoot}$ claude</div>
      <div>&nbsp;</div>
      <div>╭──────────────────────────────────────────────────╮</div>
      <div>│  Claude Code · {project.name.padEnd(34)}│</div>
      <div>│  How can I help you today?                       │</div>
      <div>╰──────────────────────────────────────────────────╯</div>
      <div>&nbsp;</div>
      <div>&gt; Refactor the sync engine to batch small writes</div>
      <div>&nbsp;</div>
      <div className="dim">● Reading src/engine.rs…</div>
      <div className="dim">● Editing src/engine.rs (+42 −18)</div>
      <div>&nbsp;</div>
      <div>
        &gt; <span className="cursor" />
      </div>
    </div>
  );
}

/* ---------- 文件（VS Code 资源管理器式） ---------- */

interface FNode {
  name: string;
  path: string; // 稳定 id（创建时的路径）
  dir?: boolean;
  sync?: "ok" | "up" | "conflict";
  size?: string;
  when?: string;
  children?: FNode[];
}

const INITIAL_TREE: FNode[] = [
  {
    name: "src", path: "src", dir: true,
    children: [
      { name: "engine.rs", path: "src/engine.rs", sync: "conflict", size: "18.4 KB", when: "2 分钟前" },
      { name: "protocol.rs", path: "src/protocol.rs", sync: "up", size: "9.1 KB", when: "刚刚" },
      { name: "watcher.rs", path: "src/watcher.rs", sync: "ok", size: "6.7 KB", when: "1 小时前" },
      { name: "lib.rs", path: "src/lib.rs", sync: "ok", size: "1.2 KB", when: "昨天" },
    ],
  },
  {
    name: "tests", path: "tests", dir: true,
    children: [
      { name: "sync_roundtrip.rs", path: "tests/sync_roundtrip.rs", sync: "ok", size: "4.5 KB", when: "3 天前" },
    ],
  },
  {
    name: "docs", path: "docs", dir: true,
    children: [
      { name: "architecture.md", path: "docs/architecture.md", sync: "ok", size: "5.9 KB", when: "上周" },
    ],
  },
  { name: ".gitignore", path: ".gitignore", sync: "ok", size: "0.1 KB", when: "上周" },
  { name: "Cargo.toml", path: "Cargo.toml", sync: "ok", size: "0.8 KB", when: "14 分钟前" },
  { name: "README.md", path: "README.md", sync: "ok", size: "2.3 KB", when: "上周" },
];

function flatten(nodes: FNode[]): FNode[] {
  return nodes.flatMap((n) => (n.children ? [n, ...flatten(n.children)] : [n]));
}
function mapTree(nodes: FNode[], fn: (n: FNode) => FNode): FNode[] {
  return nodes.map((n) => fn({ ...n, children: n.children ? mapTree(n.children, fn) : undefined }));
}
function removeFromTree(nodes: FNode[], path: string): FNode[] {
  return nodes.filter((n) => n.path !== path).map((n) => (n.children ? { ...n, children: removeFromTree(n.children, path) } : n));
}
function sortNodes(ns: FNode[]): FNode[] {
  return [...ns].sort((a, b) => (b.dir ? 1 : 0) - (a.dir ? 1 : 0) || a.name.localeCompare(b.name));
}
function insertChild(nodes: FNode[], parent: string | null, child: FNode): FNode[] {
  if (parent === null) return sortNodes([...nodes, child]);
  return nodes.map((n) =>
    n.path === parent
      ? { ...n, children: sortNodes([...(n.children ?? []), child]) }
      : n.children ? { ...n, children: insertChild(n.children, parent, child) } : n,
  );
}
/** 从根到目标节点的链路（用面包屑显示重命名后的名字） */
function findTrail(nodes: FNode[], path: string, trail: FNode[] = []): FNode[] | null {
  for (const n of nodes) {
    if (n.path === path) return [...trail, n];
    if (n.children) {
      const r = findTrail(n.children, path, [...trail, n]);
      if (r) return r;
    }
  }
  return null;
}

const FILE_BODIES: Record<string, string> = {
  rs: `use crate::protocol::Mutation;

/// Batches small writes into a single mutation frame to
/// reduce round-trips on chatty file watchers.
pub struct WriteBatcher {
    pending: Vec<Mutation>,
    max_batch: usize,
}

impl WriteBatcher {
    pub fn push(&mut self, m: Mutation) {
        self.pending.push(m);
        if self.pending.len() >= self.max_batch {
            self.flush();
        }
    }
}`,
  toml: `[package]
name = "sync-engine"
version = "0.4.2"
edition = "2021"

[dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }`,
  md: `# 架构说明

双向安全同步引擎，本地与远端各持有一份完整副本。

## 模块
- engine：写入合并与调度
- protocol：增量帧编码
- watcher：文件系统事件监听`,
};

function bodyFor(name: string): string {
  const ext = name.split(".").pop() ?? "";
  return FILE_BODIES[ext] ?? "（新文件，暂无内容）";
}

const SYNC_META = {
  ok: { icon: "✓", label: "已同步", color: "var(--green)" },
  up: { icon: "↑↓", label: "正在同步", color: "var(--blue)" },
  conflict: { icon: "⚠", label: "有冲突", color: "var(--orange)" },
};

function fileIcon(name: string, dir?: boolean, open?: boolean) {
  if (dir) return open ? "📂" : "📁";
  if (name.endsWith(".rs")) return "🦀";
  if (name.endsWith(".md")) return "📝";
  if (name.endsWith(".toml") || name.endsWith(".json")) return "⚙️";
  if (name.startsWith(".")) return "🔧";
  return "📄";
}

type Editing =
  | { mode: "new-file" | "new-folder"; parent: string | null }
  | { mode: "rename"; path: string }
  | null;

function FilesMock({ project }: { project: Project }) {
  const { showErrors, toast } = useDemo();
  const [tree, setTree] = useState<FNode[]>(INITIAL_TREE);
  const [expanded, setExpanded] = useState<Set<string>>(new Set(["src"]));
  const [rootOpen, setRootOpen] = useState(true);
  const [selected, setSelected] = useState<string | null>(null); // 树中高亮（可为文件夹）
  const [tabs, setTabs] = useState<string[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; node: FNode | null } | null>(null);
  const [editing, setEditing] = useState<Editing>(null);
  const [draft, setDraft] = useState("");
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);

  useEffect(() => {
    const t = setTimeout(() => setLoading(false), 700);
    return () => clearTimeout(t);
  }, []);

  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    window.addEventListener("click", close);
    window.addEventListener("contextmenu", close, true);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("contextmenu", close, true);
    };
  }, [menu]);

  const allFiles = useMemo(() => flatten(tree).filter((n) => !n.dir), [tree]);
  const activeTrail = active ? findTrail(tree, active) : null;
  const activeNode = activeTrail?.[activeTrail.length - 1] ?? null;
  const recent = useMemo(
    () => allFiles.filter((f) => f.when && !["昨天", "上周", "3 天前"].includes(f.when!)).slice(0, 5),
    [allFiles],
  );

  function openFile(path: string) {
    setSelected(path);
    if (!tabs.includes(path)) setTabs([...tabs, path]);
    setActive(path);
  }

  function closeTab(path: string) {
    const rest = tabs.filter((t) => t !== path);
    setTabs(rest);
    if (active === path) setActive(rest[rest.length - 1] ?? null);
  }

  function toggleDir(path: string) {
    const next = new Set(expanded);
    next.has(path) ? next.delete(path) : next.add(path);
    setExpanded(next);
  }

  /** 新建入口：优先在选中的文件夹里创建，否则根目录 */
  function startCreate(mode: "new-file" | "new-folder", parentOverride?: string | null) {
    const selTrail = selected ? findTrail(tree, selected) : null;
    const selNode = selTrail?.[selTrail.length - 1];
    const parent =
      parentOverride !== undefined
        ? parentOverride
        : selNode?.dir
          ? selNode.path
          : selTrail && selTrail.length > 1
            ? selTrail[selTrail.length - 2].path
            : null;
    if (parent) setExpanded(new Set([...expanded, parent]));
    setDraft("");
    setEditing({ mode, parent });
    setMenu(null);
  }

  function commitCreate() {
    if (!editing || editing.mode === "rename") return;
    const name = draft.trim();
    if (!name) return setEditing(null);
    const parent = editing.parent;
    const path = (parent ? parent + "/" : "") + name;
    const node: FNode =
      editing.mode === "new-folder"
        ? { name, path, dir: true, children: [] }
        : { name, path, sync: "up", size: "0 KB", when: "刚刚" };
    setTree(insertChild(tree, parent, node));
    setEditing(null);
    toast(editing.mode === "new-folder" ? `已创建文件夹 ${name}，正在同步到服务器…` : `已创建 ${name}，正在同步到服务器…`);
    if (editing.mode === "new-file") openFile(path);
  }

  function startRename(node: FNode) {
    setDraft(node.name);
    setEditing({ mode: "rename", path: node.path });
    setMenu(null);
  }

  function commitRename() {
    if (!editing || editing.mode !== "rename") return;
    const name = draft.trim();
    if (name) {
      setTree(mapTree(tree, (n) => (n.path === editing.path ? { ...n, name, when: "刚刚", sync: n.dir ? n.sync : "up" } : n)));
      toast(`已重命名为 ${name}，正在同步到服务器…`);
    }
    setEditing(null);
  }

  function deleteNode(node: FNode) {
    setMenu(null);
    const prev = tree;
    setTree(removeFromTree(tree, node.path));
    if (tabs.includes(node.path)) closeTab(node.path);
    flatten([node]).forEach((n) => tabs.includes(n.path) && closeTab(n.path));
    toast(`已删除 ${node.name}（两端同步删除）。`, "撤销", () => setTree(prev));
  }

  async function refresh() {
    setRefreshing(true);
    await new Promise((r) => setTimeout(r, 600));
    setRefreshing(false);
    toast("已刷新，与服务器一致。");
  }

  if (loading) {
    return (
      <div className="files">
        <div className="file-tree" style={{ padding: 12 }}>
          {[80, 120, 100, 90, 110].map((w, i) => (
            <div key={i} className="skeleton" style={{ height: 16, width: w, margin: "10px 6px" }} />
          ))}
        </div>
        <div className="editor" />
      </div>
    );
  }

  if (showErrors) {
    return (
      <div className="files" style={{ alignItems: "center", justifyContent: "center" }}>
        <div style={{ textAlign: "center" }}>
          <p style={{ color: "var(--red)", marginBottom: 12 }}>无法读取本地同步文件夹。</p>
          <button className="btn btn-secondary">重试</button>
        </div>
      </div>
    );
  }

  const inlineInput = (commit: () => void) => (
    <input
      className="inline-edit"
      value={draft}
      autoFocus
      placeholder={editing?.mode === "new-folder" ? "文件夹名称" : "文件名（含扩展名）"}
      onChange={(e) => setDraft(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === "Enter") commit();
        if (e.key === "Escape") setEditing(null);
      }}
      onBlur={() => setEditing(null)}
      onClick={(e) => e.stopPropagation()}
    />
  );

  function renderNodes(nodes: FNode[], depth: number) {
    return nodes.map((n) => {
      const isRenaming = editing?.mode === "rename" && editing.path === n.path;
      const open = expanded.has(n.path);
      return (
        <div key={n.path}>
          <div
            className={`file-row ${selected === n.path ? "sel" : ""} ${active === n.path ? "opened" : ""}`}
            style={{ paddingLeft: 8 + depth * 14 }}
            onClick={() => {
              setSelected(n.path);
              n.dir ? toggleDir(n.path) : openFile(n.path);
            }}
            onContextMenu={(e) => {
              e.preventDefault();
              e.stopPropagation();
              setSelected(n.path);
              setMenu({ x: e.clientX, y: e.clientY, node: n });
            }}
          >
            {Array.from({ length: depth }).map((_, i) => <span key={i} className="guide" />)}
            <span className="chev">{n.dir ? (open ? "▾" : "▸") : ""}</span>
            <span className="fic">{fileIcon(n.name, n.dir, open)}</span>
            {isRenaming ? (
              inlineInput(commitRename)
            ) : (
              <span className="fname">{n.name}</span>
            )}
            {n.sync && !isRenaming && (
              <span className="sync-ic" title={SYNC_META[n.sync].label} style={{ color: SYNC_META[n.sync].color }}>
                {SYNC_META[n.sync].icon}
              </span>
            )}
          </div>
          {n.dir && open && (
            <>
              {editing && editing.mode !== "rename" && editing.parent === n.path && (
                <div className="file-row" style={{ paddingLeft: 8 + (depth + 1) * 14 }}>
                  {Array.from({ length: depth + 1 }).map((_, i) => <span key={i} className="guide" />)}
                  <span className="chev" />
                  <span className="fic">{editing.mode === "new-folder" ? "📁" : "📄"}</span>
                  {inlineInput(commitCreate)}
                </div>
              )}
              {n.children && renderNodes(n.children, depth + 1)}
            </>
          )}
        </div>
      );
    });
  }

  return (
    <div className="files">
      <div
        className="file-tree"
        onContextMenu={(e) => {
          e.preventDefault();
          setMenu({ x: e.clientX, y: e.clientY, node: null });
        }}
      >
        <div className="explorer-head">
          <span>资源管理器</span>
        </div>
        <div className="root-row" onClick={() => setRootOpen(!rootOpen)}>
          <span className="chev">{rootOpen ? "▾" : "▸"}</span>
          <strong>{project.name.toUpperCase()}</strong>
          <span className="tools" onClick={(e) => e.stopPropagation()}>
            <button className="icon-btn" title="新建文件" onClick={() => startCreate("new-file")}>＋📄</button>
            <button className="icon-btn" title="新建文件夹" onClick={() => startCreate("new-folder")}>＋📁</button>
            <button className="icon-btn" title="刷新" onClick={refresh}>{refreshing ? "…" : "⟳"}</button>
            <button className="icon-btn" title="全部折叠" onClick={() => setExpanded(new Set())}>⊟</button>
          </span>
        </div>
        {rootOpen && (
          <>
            {editing && editing.mode !== "rename" && editing.parent === null && (
              <div className="file-row" style={{ paddingLeft: 8 }}>
                <span className="chev" />
                <span className="fic">{editing.mode === "new-folder" ? "📁" : "📄"}</span>
                {inlineInput(commitCreate)}
              </div>
            )}
            {renderNodes(tree, 0)}
          </>
        )}
      </div>

      <div className="editor">
        {tabs.length > 0 && (
          <div className="ed-tabs">
            {tabs.map((t) => {
              const trail = findTrail(tree, t);
              const node = trail?.[trail.length - 1];
              if (!node) return null;
              return (
                <div key={t} className={`ed-tab ${active === t ? "active" : ""}`} onClick={() => { setActive(t); setSelected(t); }}>
                  <span>{fileIcon(node.name)}</span>
                  <span>{node.name}</span>
                  {node.sync === "conflict" && <span style={{ color: "var(--orange)", fontSize: 10 }}>⚠</span>}
                  <span className="x" onClick={(e) => { e.stopPropagation(); closeTab(t); }}>×</span>
                </div>
              );
            })}
          </div>
        )}
        {activeNode && activeTrail ? (
          <div className="ed-body">
            <div className="crumbs">
              <span className="crumb" onClick={() => setActive(null)}>{project.name}</span>
              {activeTrail.map((seg, i) => (
                <span key={seg.path}>
                  <span className="crumb-sep">›</span>
                  <span className={i === activeTrail.length - 1 ? "crumb last" : "crumb"}>{seg.name}</span>
                </span>
              ))}
              <span style={{ marginLeft: "auto", display: "flex", gap: 8 }}>
                <button className="btn btn-ghost" style={{ padding: "2px 8px", fontSize: 12 }} onClick={() => toast("已复制路径。")}>
                  复制路径
                </button>
                <button className="btn btn-secondary" style={{ padding: "3px 10px", fontSize: 12 }} onClick={() => toast("正在用默认编辑器打开…")}>
                  用编辑器打开
                </button>
              </span>
            </div>
            <div className="file-meta">
              {activeNode.size} · 修改于 {activeNode.when} ·{" "}
              <span style={{ color: SYNC_META[activeNode.sync ?? "ok"].color }}>
                {SYNC_META[activeNode.sync ?? "ok"].icon} {SYNC_META[activeNode.sync ?? "ok"].label}
              </span>
              {activeNode.sync === "conflict" && (
                <span style={{ marginLeft: 10 }}>
                  <a href="#" onClick={(e) => e.preventDefault()}>去「冲突」页处理 →</a>
                </span>
              )}
            </div>
            <div className="code">
              {bodyFor(activeNode.name).split("\n").map((line, i) => (
                <div className="code-line" key={i}>
                  <span className="ln-no">{i + 1}</span>
                  <span className="ln-body">{line || " "}</span>
                </div>
              ))}
            </div>
          </div>
        ) : (
          <div className="ed-body">
            <div className="recent-panel">
              <h4>最近更新</h4>
              <p style={{ color: "var(--gray)", fontSize: 13, marginBottom: 14 }}>
                Claude 在服务器上改了什么，这里一目了然。点击文件查看内容。
              </p>
              {recent.map((f) => (
                <div key={f.path} className="recent-row" onClick={() => openFile(f.path)}>
                  <span>{fileIcon(f.name)}</span>
                  <span className="rp">{f.path}</span>
                  <span className="rw">{f.when}</span>
                  <span className="sync-ic" style={{ color: SYNC_META[f.sync ?? "ok"].color }}>{SYNC_META[f.sync ?? "ok"].icon}</span>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>

      {menu && (
        <div className="ctx-menu" style={{ left: menu.x, top: menu.y }} onClick={(e) => e.stopPropagation()}>
          {menu.node ? (
            <>
              {!menu.node.dir && (
                <div className="ctx-item" onClick={() => { openFile(menu.node!.path); setMenu(null); }}>打开</div>
              )}
              {menu.node.dir && (
                <>
                  <div className="ctx-item" onClick={() => startCreate("new-file", menu.node!.path)}>新建文件</div>
                  <div className="ctx-item" onClick={() => startCreate("new-folder", menu.node!.path)}>新建文件夹</div>
                </>
              )}
              <div className="ctx-sep" />
              <div className="ctx-item" onClick={() => startRename(menu.node!)}>重命名</div>
              <div className="ctx-item" onClick={() => { setMenu(null); toast("已复制路径。"); }}>复制路径</div>
              <div className="ctx-item" onClick={() => { setMenu(null); toast("正在 Finder 中显示…"); }}>在 Finder 中显示</div>
              <div className="ctx-sep" />
              <div className="ctx-item danger" onClick={() => deleteNode(menu.node!)}>删除</div>
            </>
          ) : (
            <>
              <div className="ctx-item" onClick={() => startCreate("new-file", null)}>新建文件</div>
              <div className="ctx-item" onClick={() => startCreate("new-folder", null)}>新建文件夹</div>
              <div className="ctx-sep" />
              <div className="ctx-item" onClick={() => { setMenu(null); refresh(); }}>刷新</div>
            </>
          )}
        </div>
      )}
    </div>
  );
}

/* ---------- 冲突 ---------- */

function ConflictsMock({
  conflicts,
  setConflicts,
}: {
  conflicts: Conflict[];
  setConflicts: (c: Conflict[]) => void;
}) {
  const { toast } = useDemo();
  const [sel, setSel] = useState<string | null>(conflicts[0]?.id ?? null);
  const current = conflicts.find((c) => c.id === sel) ?? conflicts[0];

  function resolve(c: Conflict, how: string) {
    const rest = conflicts.filter((x) => x.id !== c.id);
    setConflicts(rest);
    setSel(rest[0]?.id ?? null);
    toast(`已解决 ${c.file} — ${how}。`, "撤销", () => setConflicts([c, ...rest]));
  }

  if (conflicts.length === 0) {
    return (
      <div className="empty-state">
        <div className="art">✓</div>
        <h3>没有冲突，已全部同步</h3>
        <p style={{ fontSize: 14 }}>当两边同时修改同一文件时，会在这里显示供你判断。</p>
      </div>
    );
  }

  return (
    <div className="conflicts">
      <div className="conf-list">
        {conflicts.map((c) => (
          <div key={c.id} className={`conf-item ${current?.id === c.id ? "sel" : ""}`} onClick={() => setSel(c.id)}>
            <div className="f">⚠ {c.file}</div>
            <div className="meta">{c.kind} · {c.when}</div>
          </div>
        ))}
      </div>
      {current && (
        <div className="conf-detail">
          <div className="conf-actions">
            <span style={{ fontFamily: "monospace", fontSize: 13, marginRight: "auto" }}>{current.file}</span>
            <button className="btn btn-secondary" onClick={() => resolve(current, "保留本地")}>保留本地</button>
            <button className="btn btn-secondary" onClick={() => resolve(current, "保留远端")}>保留远端</button>
            <button className="btn btn-ghost" onClick={() => resolve(current, "两者都保留")}>两者都保留</button>
          </div>
          <div className="diff">
            <div className="pane">
              <h4>本地 — 2 分钟前修改</h4>
              <div className="ln">pub struct WriteBatcher {"{"}</div>
              <div className="ln add">    pending: Vec&lt;Mutation&gt;,</div>
              <div className="ln add">    max_batch: usize,</div>
              <div className="ln">    flushed_at: Instant,</div>
              <div className="ln">{"}"}</div>
            </div>
            <div className="pane">
              <h4>远端 — 3 分钟前修改</h4>
              <div className="ln">pub struct WriteBatcher {"{"}</div>
              <div className="ln del">    queue: VecDeque&lt;Mutation&gt;,</div>
              <div className="ln">    flushed_at: Instant,</div>
              <div className="ln">{"}"}</div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
