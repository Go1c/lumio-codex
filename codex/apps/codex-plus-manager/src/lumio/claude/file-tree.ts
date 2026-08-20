import type { ClaudeFileEntry } from "./types.ts";

export type ExplorerChange = "unchanged" | "untracked" | "modified";

export type ExplorerBadge = "" | "U" | "M";

export type ExplorerListing = {
  path: string;
  name: string;
  kind: "file" | "directory";
  fingerprint?: string | null;
  children?: ExplorerListing[];
};

export type ExplorerNode = {
  path: string;
  name: string;
  kind: "file" | "directory";
  depth: number;
  change: ExplorerChange;
  badge: ExplorerBadge;
  children: ExplorerNode[];
};

function indexListings(nodes: ExplorerListing[]): Map<string, ExplorerListing> {
  const map = new Map<string, ExplorerListing>();
  const walk = (list: ExplorerListing[]) => {
    for (const node of list) {
      map.set(node.path, node);
      if (node.children && node.children.length > 0) walk(node.children);
    }
  };
  walk(nodes);
  return map;
}

function parentPath(path: string): string | null {
  const index = path.lastIndexOf("/");
  return index === -1 ? null : path.slice(0, index);
}

function hasUnder(index: Map<string, ExplorerListing>, path: string): boolean {
  if (index.has(path)) return true;
  const prefix = `${path}/`;
  for (const key of index.keys()) {
    if (key.startsWith(prefix)) return true;
  }
  return false;
}

function classify(
  local: ExplorerListing | undefined,
  remote: ExplorerListing | undefined,
  localHas: boolean,
  remoteHas: boolean,
  isDir: boolean,
): { change: ExplorerChange; badge: ExplorerBadge } {
  if (localHas && remoteHas) {
    if (isDir) return { change: "unchanged", badge: "" };
    const left = local?.fingerprint ?? null;
    const right = remote?.fingerprint ?? null;
    if (left != null && right != null && left !== right) {
      return { change: "modified", badge: "M" };
    }
    return { change: "unchanged", badge: "" };
  }
  return { change: "untracked", badge: "U" };
}

function sortNodes(nodes: ExplorerNode[]): void {
  nodes.sort((left, right) => {
    if (left.kind !== right.kind) return left.kind === "directory" ? -1 : 1;
    return left.name.localeCompare(right.name);
  });
}

export function mergeExplorerTrees(
  local: ExplorerListing[],
  remote: ExplorerListing[],
): ExplorerNode[] {
  const locals = indexListings(local);
  const remotes = indexListings(remote);
  const paths = new Set<string>([...locals.keys(), ...remotes.keys()]);
  for (const path of [...paths]) {
    let parent = parentPath(path);
    while (parent) {
      paths.add(parent);
      parent = parentPath(parent);
    }
  }

  const byParent = new Map<string | null, string[]>();
  for (const path of paths) {
    const parent = parentPath(path);
    const siblings = byParent.get(parent) ?? [];
    siblings.push(path);
    byParent.set(parent, siblings);
  }

  const build = (parent: string | null, depth: number): ExplorerNode[] => {
    const childPaths = byParent.get(parent) ?? [];
    const nodes: ExplorerNode[] = childPaths.map((path) => {
      const localNode = locals.get(path);
      const remoteNode = remotes.get(path);
      const nested = (byParent.get(path) ?? []).length > 0;
      const isDir =
        nested || localNode?.kind === "directory" || remoteNode?.kind === "directory";
      const name =
        localNode?.name ??
        remoteNode?.name ??
        (path.includes("/") ? path.slice(path.lastIndexOf("/") + 1) : path);
      const localHas = hasUnder(locals, path);
      const remoteHas = hasUnder(remotes, path);
      const { change, badge } = classify(localNode, remoteNode, localHas, remoteHas, isDir);
      const children = isDir ? build(path, depth + 1) : [];
      return {
        path,
        name,
        kind: isDir ? "directory" : "file",
        depth,
        change,
        badge,
        children,
      };
    });
    sortNodes(nodes);
    return nodes;
  };

  return build(null, 0);
}

export function flattenExplorer(nodes: ExplorerNode[]): ExplorerNode[] {
  const out: ExplorerNode[] = [];
  const walk = (list: ExplorerNode[]) => {
    for (const node of list) {
      out.push(node);
      if (node.children.length > 0) walk(node.children);
    }
  };
  walk(nodes);
  return out;
}

export function flattenVisible(nodes: ExplorerNode[], expanded: Set<string>): ExplorerNode[] {
  const out: ExplorerNode[] = [];
  const walk = (list: ExplorerNode[]) => {
    for (const node of list) {
      out.push(node);
      if (node.kind === "directory" && expanded.has(node.path)) walk(node.children);
    }
  };
  walk(nodes);
  return out;
}

export function listingsFromEntries(entries: ClaudeFileEntry[]): ExplorerListing[] {
  return entries.map(entryToListing);
}

function entryToListing(entry: ClaudeFileEntry): ExplorerListing {
  return {
    path: entry.path,
    name: entry.name,
    kind: entry.kind,
    fingerprint: fingerprintFromEntry(entry),
    children: entry.children?.map(entryToListing),
  };
}

export function fingerprintFromEntry(entry: ClaudeFileEntry): string | null {
  if (entry.fingerprint != null && entry.fingerprint !== "") return entry.fingerprint;
  if (entry.size != null) return `size:${entry.size}`;
  return null;
}

export function sideForExplorerPath(
  path: string,
  files: ClaudeFileEntry[],
): "local" | "remote" {
  const find = (nodes: ClaudeFileEntry[]): boolean => {
    for (const node of nodes) {
      if (node.path === path) return true;
      if (node.children && find(node.children)) return true;
    }
    return false;
  };
  const local = files.filter((file) => file.side !== "remote");
  if (find(local)) return "local";
  return "remote";
}
