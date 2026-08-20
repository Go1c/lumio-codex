export const CONTENT_SEARCH_HINT = "输入要在文件中搜索的内容";

export type ExplorerFilterKind = "dir" | "file";

export type ExplorerFilterNode = {
  path: string;
  kind: ExplorerFilterKind;
  text?: string;
};

export type ExplorerMatchFlags = {
  case: boolean;
  word: boolean;
  regex: boolean;
};

export type ExplorerFilterMode = "name" | "content";

function escapeRe(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function basename(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

export function compileExplorerMatcher(
  query: string,
  flags: ExplorerMatchFlags,
): RegExp | null | "bad" {
  if (!query) return null;
  let source = flags.regex ? query : escapeRe(query);
  if (flags.word) source = `\\b(?:${source})\\b`;
  try {
    return new RegExp(source, flags.case ? "g" : "gi");
  } catch {
    return "bad";
  }
}

export function compileExplorerGlobs(value: string): RegExp[] {
  return value
    .split(",")
    .map((one) => one.trim())
    .filter(Boolean)
    .map((one) => {
      const pattern = one.includes("/") ? one : `**/${one}`;
      const body = escapeRe(pattern)
        .replace(/\\\*\\\*\//g, "\u0000")
        .replace(/\\\*\\\*/g, "\u0001")
        .replace(/\\\*/g, "[^/]*")
        .replace(/\\\?/g, "[^/]")
        .replace(/\u0000/g, "(?:.*/)?")
        .replace(/\u0001/g, ".*");
      try {
        return new RegExp(`^${body}$`);
      } catch {
        return null;
      }
    })
    .filter((glob): glob is RegExp => glob !== null);
}

function explorerHits(text: string | undefined, re: RegExp): boolean {
  if (!text) return false;
  re.lastIndex = 0;
  return re.test(text);
}

export function sortExplorerNodes<T extends { path: string; kind: ExplorerFilterKind }>(
  nodes: T[],
): T[] {
  return [...nodes].sort((a, b) => {
    const ap = a.path.split("/");
    const bp = b.path.split("/");
    for (let i = 0; i < Math.max(ap.length, bp.length); i += 1) {
      if (ap[i] === bp[i]) continue;
      if (ap[i] === undefined) return -1;
      if (bp[i] === undefined) return 1;
      const aLeaf = i === ap.length - 1 && a.kind === "file";
      const bLeaf = i === bp.length - 1 && b.kind === "file";
      if (aLeaf !== bLeaf) return aLeaf ? 1 : -1;
      return ap[i].localeCompare(bp[i], "zh-Hans-CN", { sensitivity: "base" });
    }
    return 0;
  });
}

export function filterExplorerNodes<T extends ExplorerFilterNode>(
  nodes: T[],
  input: {
    mode: ExplorerFilterMode;
    query: string;
    flags: ExplorerMatchFlags;
    include?: string;
    exclude?: string;
  },
): T[] | "bad" {
  const re = compileExplorerMatcher(input.query, input.flags);
  if (re === "bad") return "bad";
  if (input.mode === "content" && !input.query) return [];
  const sorted = sortExplorerNodes(nodes);
  if (input.mode === "content") {
    if (re === null) return [];
    const include = compileExplorerGlobs(input.include ?? "");
    const exclude = compileExplorerGlobs(input.exclude ?? "");
    return sorted.filter(
      (node) =>
        node.kind === "file" &&
        explorerHits(node.text, re) &&
        (include.length === 0 || include.some((glob) => glob.test(node.path))) &&
        !exclude.some((glob) => glob.test(node.path)),
    );
  }
  if (!input.query || re === null) return sorted;
  return sorted.filter((node) => explorerHits(basename(node.path), re));
}
