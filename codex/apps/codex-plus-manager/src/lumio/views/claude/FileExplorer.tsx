import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type MouseEvent,
  type ReactNode,
} from "react";

import { previewClaudeFile } from "../../claude/api.ts";
import {
  compileExplorerMatcher,
  filterExplorerNodes,
  sortExplorerNodes,
  type ExplorerFilterMode,
  type ExplorerMatchFlags,
} from "../../claude/explorer-filter.ts";
import { fileIconKind, type FileIconKind } from "../../claude/file-icons.ts";
import {
  flattenVisible,
  listingsFromEntries,
  mergeExplorerTrees,
  sideForExplorerPath,
  type ExplorerBadge,
  type ExplorerChange,
  type ExplorerNode,
} from "../../claude/file-tree.ts";
import { refreshClaudeFiles } from "../../claude/session.ts";
import { getClaudeState, projectPassword, subscribeClaudeStore } from "../../claude/store.ts";
import type { ClaudeFilePreview, ClaudeProject, ClaudeState } from "../../claude/types.ts";

type FxKind = "dir" | "file";

type FxRow = {
  path: string;
  name: string;
  kind: FxKind;
  depth: number;
  change: ExplorerChange;
  badge: ExplorerBadge;
  text?: string;
};

type CtxItem =
  | { sep: true }
  | { id: string; label: string; keys?: string; danger?: boolean; only?: "md"; icon: keyof typeof FX_ICONS };

const FX_CTX_ITEMS: CtxItem[] = [
  { id: "new-file", label: "新文件", icon: "filePlus" },
  { id: "new-folder", label: "新建文件夹", icon: "folderPlus" },
  { sep: true },
  { id: "copy", label: "复制", icon: "copy" },
  { id: "copy-path", label: "复制路径", keys: "⌘⌥C", icon: "clipboard" },
  { id: "copy-rel", label: "复制相对路径", keys: "⌘⌥⇧C", icon: "clipboard" },
  { id: "duplicate", label: "复制", icon: "duplicate" },
  { id: "view", label: "查看文件", icon: "fileEye" },
  { id: "browser", label: "在内置浏览器中打开", icon: "globe" },
  { id: "md", label: "打开 Markdown 预览", only: "md", icon: "eye" },
  { id: "finder", label: "在 Finder 中显示", icon: "external" },
  { sep: true },
  { id: "rename", label: "重命名", keys: "↵", icon: "pencil" },
  { id: "delete", label: "删除", keys: "⌘Backspace, Delete", danger: true, icon: "trash" },
];

export function FileExplorer({
  project,
  files,
}: {
  project: ClaudeProject;
  files: ClaudeState["filesByProject"][string];
}) {
  const paneRef = useRef<HTMLDivElement>(null);
  const headRef = useRef<HTMLElement>(null);
  const ctxRef = useRef<HTMLDivElement>(null);
  const seededFor = useRef<string | null>(null);
  const [preview, setPreview] = useState<ClaudeFilePreview | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [query, setQuery] = useState("");
  const [mode, setMode] = useState<ExplorerFilterMode>("name");
  const [flags, setFlags] = useState<ExplorerMatchFlags>({ case: false, word: false, regex: false });
  const [include, setInclude] = useState("");
  const [exclude, setExclude] = useState("");
  const [onlyChanged, setOnlyChanged] = useState(false);
  const [moreOpen, setMoreOpen] = useState(false);
  const [picked, setPicked] = useState<string | null>(null);
  const [spinning, setSpinning] = useState(false);
  const [conflictPaths, setConflictPaths] = useState<Set<string>>(() => readConflictPaths(project.id));
  const [ctx, setCtx] = useState<{ x: number; y: number; path: string; ext: string } | null>(null);
  const [contentByPath, setContentByPath] = useState<Map<string, string>>(() => new Map());
  const contentRef = useRef(contentByPath);
  contentRef.current = contentByPath;

  const tree = useMemo(() => {
    const local = listingsFromEntries(files.filter((file) => file.side !== "remote"));
    const remote = listingsFromEntries(files.filter((file) => file.side === "remote"));
    return mergeExplorerTrees(local, remote);
  }, [files]);

  const dirPaths = useMemo(() => collectDirPaths(tree), [tree]);
  const catalogPaths = useMemo(
    () =>
      flattenVisible(tree, new Set(dirPaths))
        .filter((node) => node.kind === "file")
        .map((node) => node.path),
    [dirPaths, tree],
  );

  useEffect(() => {
    if (seededFor.current === project.id) return;
    if (dirPaths.length === 0 && files.length === 0) return;
    seededFor.current = project.id;
    setExpanded(new Set(dirPaths));
  }, [dirPaths, files.length, project.id]);

  useEffect(() => {
    contentRef.current = new Map();
    setContentByPath(new Map());
  }, [project.id]);

  useEffect(() => {
    if (mode !== "content") return;
    if (!query.trim()) return;
    const pending = catalogPaths.filter((path) => !contentRef.current.has(path));
    if (pending.length === 0) return undefined;
    let cancelled = false;
    void Promise.all(
      pending.map(async (path) => {
        try {
          const preview = await previewClaudeFile({
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
            side: "local",
          });
          if (!preview || preview.binary || preview.tooLarge) return null;
          return [path, preview.content] as const;
        } catch {
          return null;
        }
      }),
    ).then((rows) => {
      if (cancelled) return;
      setContentByPath((current) => {
        let changed = false;
        const next = new Map(current);
        for (const row of rows) {
          if (!row) continue;
          if (next.get(row[0]) === row[1]) continue;
          next.set(row[0], row[1]);
          changed = true;
        }
        return changed ? next : current;
      });
    });
    return () => {
      cancelled = true;
    };
  }, [catalogPaths, mode, project, query]);

  useEffect(() => {
    const read = () => setConflictPaths(readConflictPaths(project.id));
    read();
    return subscribeClaudeStore(read);
  }, [project.id]);

  useEffect(() => {
    const onPointer = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (moreOpen && headRef.current && target && !headRef.current.contains(target)) setMoreOpen(false);
      if (ctx && ctxRef.current && target && !ctxRef.current.contains(target)) setCtx(null);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      setMoreOpen(false);
      setCtx(null);
    };
    document.addEventListener("pointerdown", onPointer);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onPointer);
      document.removeEventListener("keydown", onKey);
    };
  }, [ctx, moreOpen]);

  const allRows = useMemo(() => {
    const catalog = flattenVisible(tree, new Set(dirPaths));
    return sortExplorerNodes(catalog.map((node) => toRow(node, contentByPath)));
  }, [contentByPath, dirPaths, tree]);

  const treeRows = useMemo(() => {
    return sortExplorerNodes(flattenVisible(tree, expanded).map((node) => toRow(node, contentByPath)));
  }, [contentByPath, expanded, tree]);

  const shown = useMemo(() => {
    const trimmed = query.trim();
    if (mode === "content" && !trimmed) return "hint" as const;
    const filtered = filterExplorerNodes(allRows, {
      mode,
      query: trimmed,
      flags,
      include,
      exclude,
    });
    if (filtered === "bad") return "bad" as const;
    if (mode === "content" || trimmed) return filtered;
    if (onlyChanged) {
      return filtered.filter((row) => row.kind === "file" && tagFor(row, conflictPaths));
    }
    return treeRows;
  }, [allRows, conflictPaths, exclude, flags, include, mode, onlyChanged, query, treeRows]);

  const matcher = useMemo(() => {
    const compiled = compileExplorerMatcher(query.trim(), flags);
    return compiled === "bad" ? null : compiled;
  }, [flags, query]);

  const flat = Boolean(query.trim()) || onlyChanged || mode === "content";
  const rows = shown === "hint" || shown === "bad" ? [] : shown;

  const openFile = (path: string) => {
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

  const toggleDir = (path: string) => {
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  const onRowClick = (row: FxRow) => {
    setPicked(row.path);
    setCtx(null);
    if (row.kind === "dir" && !flat) {
      toggleDir(row.path);
      return;
    }
    if (row.kind === "file") openFile(row.path);
  };

  const onRowContext = (event: MouseEvent<HTMLButtonElement>, row: FxRow) => {
    event.preventDefault();
    event.stopPropagation();
    setPicked(row.path);
    setMoreOpen(false);
    const box = paneRef.current?.getBoundingClientRect();
    let x = event.clientX - (box?.left ?? 0);
    let y = event.clientY - (box?.top ?? 0);
    const width = box?.width ?? 280;
    const height = box?.height ?? 480;
    if (x + 240 > width) x = Math.max(4, width - 244);
    if (y + 380 > height) y = Math.max(4, height - 384);
    setCtx({ x, y, path: row.path, ext: fileExt(row.path) });
  };

  const closeCtx = () => setCtx(null);

  const onCtxAction = (id: string) => {
    if (!ctx) return;
    if (id === "view" || id === "md") openFile(ctx.path);
    if (id === "copy-path") void writeClipboard(joinProjectPath(project.localRoot, ctx.path));
    if (id === "copy-rel" || id === "copy") void writeClipboard(id === "copy" ? basename(ctx.path) : ctx.path);
    closeCtx();
  };

  const isName = mode === "name";
  const emptyTree = allRows.length === 0;
  const conflictCount = conflictPaths.size;
  const foot =
    shown === "hint" || shown === "bad" || emptyTree
      ? ""
      : query.trim()
        ? `${rows.length} 个结果 · ${mode === "name" ? "按名称" : "按内容"}`
        : onlyChanged
          ? `只看有改动的 · ${rows.length} 个文件`
          : conflictCount > 0
            ? `本机与服务器合并显示 · ${conflictCount} 个文件有冲突，在底栏处理`
            : "本机与服务器合并显示。角标是两边的差别。";

  return (
    <div className="lumio-claude-files lumio-claude-fx" ref={paneRef}>
      <header className="lumio-claude-fx-head" ref={headRef}>
        <h3>{project.name}</h3>
        <button
          className="lumio-claude-fx-icon"
          data-tip="全部折叠"
          type="button"
          aria-label="全部折叠"
          onClick={() => {
            setExpanded(new Set());
            setPicked(null);
            setMoreOpen(false);
          }}
        >
          <span className="lumio-claude-fx-ico">{FX_ICONS.collapse}</span>
        </button>
        <button
          className={`lumio-claude-fx-icon${spinning ? " is-spin" : ""}`}
          data-tip="刷新文件管理器"
          type="button"
          aria-label="刷新文件管理器"
          onAnimationEnd={() => setSpinning(false)}
          onClick={() => {
            setSpinning(false);
            requestAnimationFrame(() => setSpinning(true));
            void refreshClaudeFiles(project.id);
          }}
        >
          <span className="lumio-claude-fx-ico">{FX_ICONS.refresh}</span>
        </button>
        <button
          className="lumio-claude-fx-icon"
          data-tip="更多"
          type="button"
          aria-label="更多"
          aria-expanded={moreOpen}
          onClick={(event) => {
            event.stopPropagation();
            setMoreOpen((open) => !open);
            setCtx(null);
          }}
        >
          <span className="lumio-claude-fx-ico">{FX_ICONS.more}</span>
        </button>
        <div className="lumio-claude-fx-menu" hidden={!moreOpen} role="menu">
          <button
            type="button"
            role="menuitemcheckbox"
            aria-checked={onlyChanged}
            onClick={() => {
              setOnlyChanged((value) => !value);
              setMoreOpen(false);
            }}
          >
            <span className="tick">{onlyChanged ? "✓" : ""}</span>
            只看有改动的
          </button>
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              setExpanded(new Set(dirPaths));
              setMoreOpen(false);
            }}
          >
            <span className="tick" />
            全部展开
          </button>
          <hr />
          <button type="button" role="menuitem" onClick={() => setMoreOpen(false)}>
            <span className="tick" />
            在本机打开项目文件夹
          </button>
        </div>
      </header>

      <div className="lumio-claude-fx-search">
        <span className="lumio-claude-fx-ico">{isName ? FX_ICONS.filter : FX_ICONS.search}</span>
        <input
          type="search"
          value={query}
          placeholder={isName ? "查找文件" : "搜索"}
          aria-label={isName ? "查找文件" : "在文件中搜索"}
          onChange={(event) => setQuery(event.target.value)}
        />
        <span className="lumio-claude-fx-flags" hidden={isName}>
          <button
            className={`lumio-claude-fx-flag${flags.case ? " is-on" : ""}`}
            data-tip="区分大小写"
            type="button"
            aria-pressed={flags.case}
            onClick={() => setFlags((current) => ({ ...current, case: !current.case }))}
          >
            Aa
          </button>
          <button
            className={`lumio-claude-fx-flag${flags.word ? " is-on" : ""}`}
            data-tip="全字匹配"
            type="button"
            aria-pressed={flags.word}
            onClick={() => setFlags((current) => ({ ...current, word: !current.word }))}
          >
            <u>ab</u>
          </button>
          <button
            className={`lumio-claude-fx-flag${flags.regex ? " is-on" : ""}`}
            data-tip="使用正则表达式"
            type="button"
            aria-pressed={flags.regex}
            onClick={() => setFlags((current) => ({ ...current, regex: !current.regex }))}
          >
            .*
          </button>
        </span>
      </div>

      <div className="lumio-claude-fx-modes" role="tablist" aria-label="检索范围">
        <button
          className={isName ? "is-on" : ""}
          data-fx-mode="name"
          role="tab"
          type="button"
          aria-selected={isName}
          onClick={() => setMode("name")}
        >
          名称
        </button>
        <button
          className={!isName ? "is-on" : ""}
          data-fx-mode="content"
          role="tab"
          type="button"
          aria-selected={!isName}
          onClick={() => setMode("content")}
        >
          内容
        </button>
      </div>

      <div className="lumio-claude-fx-globs" hidden={isName}>
        <label>
          要包含的文件
          <input
            type="text"
            value={include}
            placeholder="要包含的文件（例如 *.ts、src/**）"
            onChange={(event) => setInclude(event.target.value)}
          />
        </label>
        <label>
          要排除的文件
          <input
            type="text"
            value={exclude}
            placeholder="要排除的文件（例如 *.min.js、dist）"
            onChange={(event) => setExclude(event.target.value)}
          />
        </label>
      </div>

      <div
        className="lumio-claude-fx-tree"
        role="tree"
        onContextMenu={(event) => event.preventDefault()}
      >
        {shown === "hint" ? (
          <p className="lumio-claude-fx-hint">输入要在文件中搜索的内容</p>
        ) : shown === "bad" ? (
          <p className="lumio-claude-fx-empty">正则写法有问题，改一下再看。</p>
        ) : emptyTree ? (
          <p className="lumio-claude-fx-empty">还没有同步下来的文件。</p>
        ) : rows.length === 0 ? (
          <p className="lumio-claude-fx-empty">
            {query.trim()
              ? `没有匹配「${query.trim()}」的${mode === "name" ? "文件名" : "文件内容"}。`
              : "两边一模一样，没有改动。"}
          </p>
        ) : (
          rows.map((row) => {
            const open = row.kind === "dir" && expanded.has(row.path);
            const tag = tagFor(row, conflictPaths);
            const depth = flat ? 0 : row.depth;
            const label = flat ? row.path : row.name;
            const glyph = fileIconKind(row.path, row.kind, open);
            return (
              <div key={row.path}>
                <button
                  className={`lumio-claude-file-row lumio-claude-fx-row${
                    row.kind === "dir" ? " is-dir" : ""
                  }${open ? " is-open" : ""}${row.path === picked ? " is-on" : ""} is-${row.change}`}
                  style={{ "--depth": String(depth) } as CSSProperties}
                  type="button"
                  role="treeitem"
                  aria-expanded={row.kind === "dir" ? open : undefined}
                  onClick={() => onRowClick(row)}
                  onContextMenu={(event) => onRowContext(event, row)}
                >
                  <span className="lumio-claude-fx-chev" aria-hidden="true">
                    {row.kind === "dir" ? FX_ICONS.chev : null}
                  </span>
                  <span className="lumio-claude-fx-glyph">{FILE_GLYPHS[glyph]}</span>
                  <span className="lumio-claude-fx-name">
                    {mode === "name" && query.trim() ? (
                      <Highlight text={label} matcher={matcher} />
                    ) : (
                      label
                    )}
                  </span>
                  {tag ? <span className={`lumio-claude-fx-tag ${tag.tone}`}>{tag.label}</span> : null}
                </button>
                {mode === "content" && row.text ? (
                  <span className="lumio-claude-fx-hit" style={{ "--depth": String(depth) } as CSSProperties}>
                    <Highlight text={row.text} matcher={matcher} />
                  </span>
                ) : null}
              </div>
            );
          })
        )}
      </div>

      <p className="lumio-claude-fx-foot">{foot}</p>

      {preview ? (
        <pre className="lumio-claude-preview">
          {preview.tooLarge
            ? "文件太大，没法在这里预览。"
            : preview.binary
              ? "这是二进制文件，没法预览。"
              : preview.content}
        </pre>
      ) : null}

      {ctx ? (
        <div
          className="lumio-claude-fx-ctx"
          ref={ctxRef}
          hidden={false}
          role="menu"
          style={{ left: ctx.x, top: ctx.y }}
        >
          {FX_CTX_ITEMS.filter((item) => !("only" in item && item.only) || item.only === ctx.ext).map((item, index) =>
            "sep" in item && item.sep ? (
              <hr key={`sep-${index}`} />
            ) : (
              <button
                key={"id" in item ? item.id : index}
                type="button"
                role="menuitem"
                className={"danger" in item && item.danger ? "is-danger" : undefined}
                onClick={() => onCtxAction("id" in item ? item.id : "")}
              >
                <span className="ico lumio-claude-fx-ico">
                  {"icon" in item ? FX_ICONS[item.icon] : null}
                </span>
                <span className="lbl">{"label" in item ? item.label : ""}</span>
                {"keys" in item && item.keys ? <span className="keys">{item.keys}</span> : null}
              </button>
            ),
          )}
        </div>
      ) : null}
    </div>
  );
}

function tagFor(
  row: { path: string; badge: ExplorerBadge },
  conflicts: Set<string>,
): { label: "已改" | "新增" | "冲突"; tone: "is-changed" | "is-added" | "is-conflict" } | null {
  if (conflicts.has(row.path)) return { label: "冲突", tone: "is-conflict" };
  if (row.badge === "U") return { label: "新增", tone: "is-added" };
  if (row.badge === "M") return { label: "已改", tone: "is-changed" };
  return null;
}

function toRow(node: ExplorerNode, texts: Map<string, string>): FxRow {
  return {
    path: node.path,
    name: node.name,
    kind: node.kind === "directory" ? "dir" : "file",
    depth: node.depth,
    change: node.change,
    badge: node.badge,
    text: texts.get(node.path),
  };
}

function collectDirPaths(nodes: ExplorerNode[]): string[] {
  const paths: string[] = [];
  const walk = (list: ExplorerNode[]) => {
    for (const node of list) {
      if (node.kind !== "directory") continue;
      paths.push(node.path);
      walk(node.children);
    }
  };
  walk(nodes);
  return paths;
}

function readConflictPaths(projectId: string): Set<string> {
  const list = getClaudeState().conflictsByProject[projectId] ?? [];
  return new Set(list.map((item) => item.path));
}

function basename(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

function fileExt(path: string): string {
  const name = basename(path);
  const dot = name.lastIndexOf(".");
  return dot >= 0 ? name.slice(dot + 1).toLowerCase() : "";
}

function joinProjectPath(root: string, rel: string): string {
  if (!rel) return root;
  return `${root.replace(/\/+$/, "")}/${rel}`;
}

async function writeClipboard(value: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(value);
  } catch {
    /* no fake success toast */
  }
}

function Highlight({ text, matcher }: { text: string; matcher: RegExp | null }) {
  if (!matcher) return text;
  const parts: ReactNode[] = [];
  let last = 0;
  matcher.lastIndex = 0;
  let match = matcher.exec(text);
  let index = 0;
  while (match) {
    if (match[0] === "") {
      matcher.lastIndex += 1;
      match = matcher.exec(text);
      continue;
    }
    if (match.index > last) parts.push(text.slice(last, match.index));
    parts.push(<b key={index}>{match[0]}</b>);
    last = match.index + match[0].length;
    index += 1;
    if (!matcher.global) break;
    match = matcher.exec(text);
  }
  parts.push(text.slice(last));
  return parts;
}

function FxSvg({
  children,
  fill = "none",
  width = "1.2",
}: {
  children: ReactNode;
  fill?: string;
  width?: string;
}) {
  return (
    <svg
      viewBox="0 0 16 16"
      fill={fill}
      stroke={fill === "currentColor" ? "none" : "currentColor"}
      strokeWidth={width}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {children}
    </svg>
  );
}

const FX_ICONS = {
  chev: (
    <FxSvg width="1.6">
      <path d="M6 3.5 10.5 8 6 12.5" />
    </FxSvg>
  ),
  collapse: (
    <FxSvg width="1.3">
      <path d="M6.5 3.5h8M6.5 8h8M6.5 12.5h8" />
      <path d="M1.5 5 3 6.5 4.5 5" />
      <path d="M1.5 11 3 9.5 4.5 11" />
    </FxSvg>
  ),
  refresh: (
    <FxSvg width="1.3">
      <path d="M13.6 8a5.6 5.6 0 1 1-1.7-4" />
      <path d="M13.9 2.2v3.2h-3.2" />
    </FxSvg>
  ),
  more: (
    <FxSvg fill="currentColor" width="0">
      <circle cx="3.4" cy="8" r="1.2" />
      <circle cx="8" cy="8" r="1.2" />
      <circle cx="12.6" cy="8" r="1.2" />
    </FxSvg>
  ),
  filter: (
    <FxSvg width="1.3">
      <path d="M2.5 4h11M4.5 8h7M6.5 12h3" />
    </FxSvg>
  ),
  search: (
    <FxSvg width="1.4">
      <circle cx="7" cy="7" r="4.4" />
      <path d="m10.4 10.4 3.1 3.1" />
    </FxSvg>
  ),
  filePlus: (
    <FxSvg>
      <path d="M9.3 1.9H4.5c-.7 0-1.2.5-1.2 1.2v9.8c0 .7.5 1.2 1.2 1.2h7c.7 0 1.2-.5 1.2-1.2V5.3L9.3 1.9Z" />
      <path d="M9.2 2v3.4h3.4" />
      <path d="M8 8.4v3.2M6.4 10h3.2" />
    </FxSvg>
  ),
  folderPlus: (
    <FxSvg>
      <path d="M1.6 4.3c0-.7.5-1.2 1.2-1.2h2.5c.4 0 .7.1.9.4l.8 1h5.2c.7 0 1.2.5 1.2 1.2v6.1c0 .7-.5 1.2-1.2 1.2H2.8c-.7 0-1.2-.5-1.2-1.2V4.3Z" />
      <path d="M8 7.6v3.4M6.3 9.3h3.4" />
    </FxSvg>
  ),
  copy: (
    <FxSvg>
      <rect x="5.6" y="5.6" width="8" height="8" rx="1.4" />
      <path d="M11 5.6V3.8c0-.8-.6-1.4-1.4-1.4H3.8c-.8 0-1.4.6-1.4 1.4v5.8c0 .8.6 1.4 1.4 1.4h1.8" />
    </FxSvg>
  ),
  clipboard: (
    <FxSvg>
      <rect x="4.4" y="2.6" width="7.2" height="11" rx="1.4" />
      <path d="M6.4 2.6V2c0-.4.3-.7.7-.7h1.8c.4 0 .7.3.7.7v.6" />
      <path d="M6.6 7.2h2.8M6.6 9.8h2.8" />
    </FxSvg>
  ),
  duplicate: (
    <FxSvg>
      <path d="M6.4 4.6h5.4c.8 0 1.4.6 1.4 1.4v7c0 .8-.6 1.4-1.4 1.4H6.4c-.8 0-1.4-.6-1.4-1.4V6c0-.8.6-1.4 1.4-1.4Z" />
      <path d="M3.2 11V3.6c0-.8.6-1.4 1.4-1.4h5.2" />
    </FxSvg>
  ),
  fileEye: (
    <FxSvg>
      <path d="M9.3 1.9H4.5c-.7 0-1.2.5-1.2 1.2v9.8c0 .7.5 1.2 1.2 1.2h7c.7 0 1.2-.5 1.2-1.2V5.3L9.3 1.9Z" />
      <path d="M9.2 2v3.4h3.4" />
    </FxSvg>
  ),
  globe: (
    <FxSvg>
      <circle cx="8" cy="8" r="6.1" />
      <path d="M1.9 8h12.2" />
      <path d="M8 1.9c1.7 1.7 2.6 3.8 2.6 6.1S9.7 12.4 8 14.1C6.3 12.4 5.4 10.3 5.4 8S6.3 3.6 8 1.9Z" />
    </FxSvg>
  ),
  eye: (
    <FxSvg>
      <path d="M1.4 8S3.8 3.6 8 3.6 14.6 8 14.6 8 12.2 12.4 8 12.4 1.4 8 1.4 8Z" />
      <circle cx="8" cy="8" r="1.9" />
    </FxSvg>
  ),
  external: (
    <FxSvg>
      <path d="M9.4 2.4h4.2v4.2" />
      <path d="m13.6 2.4-6 6" />
      <path d="M12 9.6v3.2c0 .5-.4.9-.9.9H3.3c-.5 0-.9-.4-.9-.9V5c0-.5.4-.9.9-.9h3.2" />
    </FxSvg>
  ),
  pencil: (
    <FxSvg>
      <path d="M11.2 2.6a1.6 1.6 0 0 1 2.2 2.2l-7.5 7.5-3 .8.8-3 7.5-7.5Z" />
    </FxSvg>
  ),
  trash: (
    <FxSvg>
      <path d="M2.6 4.4h10.8" />
      <path d="M6.2 4.4V2.9c0-.4.3-.7.7-.7h2.2c.4 0 .7.3.7.7v1.5" />
      <path d="M4.2 4.4l.6 8.4c0 .5.5.9 1 .9h4.4c.5 0 1-.4 1-.9l.6-8.4" />
    </FxSvg>
  ),
};

const FILE_GLYPHS: Record<FileIconKind, ReactNode> = {
  dir: (
    <FxSvg>
      <path d="M1.6 4.3c0-.7.5-1.2 1.2-1.2h2.5c.4 0 .7.1.9.4l.8 1h5.2c.7 0 1.2.5 1.2 1.2v6.1c0 .7-.5 1.2-1.2 1.2H2.8c-.7 0-1.2-.5-1.2-1.2V4.3Z" />
    </FxSvg>
  ),
  dirOpen: (
    <FxSvg>
      <path d="M1.6 4.3c0-.7.5-1.2 1.2-1.2h2.5c.4 0 .7.1.9.4l.8 1h5.2c.7 0 1.2.5 1.2 1.2v.9" />
      <path d="M1.6 6.6h12.8l-1.1 5.4c-.1.6-.6 1-1.2 1H3.9c-.6 0-1.1-.4-1.2-1L1.6 6.6Z" />
    </FxSvg>
  ),
  file: (
    <FxSvg>
      <path d="M9.3 1.9H4.5c-.7 0-1.2.5-1.2 1.2v9.8c0 .7.5 1.2 1.2 1.2h7c.7 0 1.2-.5 1.2-1.2V5.3L9.3 1.9Z" />
      <path d="M9.2 2v3.4h3.4" />
    </FxSvg>
  ),
  fileText: (
    <FxSvg>
      <path d="M9.3 1.9H4.5c-.7 0-1.2.5-1.2 1.2v9.8c0 .7.5 1.2 1.2 1.2h7c.7 0 1.2-.5 1.2-1.2V5.3L9.3 1.9Z" />
      <path d="M9.2 2v3.4h3.4" />
      <path d="M5.4 8.2h5.2M5.4 10.4h5.2M5.4 12h3.2" />
    </FxSvg>
  ),
  fileCode: (
    <FxSvg>
      <path d="M9.3 1.9H4.5c-.7 0-1.2.5-1.2 1.2v9.8c0 .7.5 1.2 1.2 1.2h7c.7 0 1.2-.5 1.2-1.2V5.3L9.3 1.9Z" />
      <path d="M9.2 2v3.4h3.4" />
      <path d="M6.6 8.9 5.2 10.3l1.4 1.4M9.4 8.9l1.4 1.4-1.4 1.4" />
    </FxSvg>
  ),
  fileConf: (
    <FxSvg>
      <path d="M9.3 1.9H4.5c-.7 0-1.2.5-1.2 1.2v9.8c0 .7.5 1.2 1.2 1.2h7c.7 0 1.2-.5 1.2-1.2V5.3L9.3 1.9Z" />
      <path d="M9.2 2v3.4h3.4" />
      <path d="M7 8.9c-.9 0-.9 1.4-1.7 1.4.8 0 .8 1.4 1.7 1.4M9 8.9c.9 0 .9 1.4 1.7 1.4-.8 0-.8 1.4-1.7 1.4" />
    </FxSvg>
  ),
};
