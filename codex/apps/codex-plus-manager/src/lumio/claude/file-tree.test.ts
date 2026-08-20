import assert from "node:assert/strict";
import test from "node:test";

import { flattenExplorer, listingsFromEntries, mergeExplorerTrees } from "./file-tree.ts";
import type { ExplorerListing } from "./file-tree.ts";
import { readAllClaudeCss, readAllClaudeViews } from "./read-claude-views.ts";
import type { ClaudeFileEntry } from "./types.ts";

function dir(
  path: string,
  children: ExplorerListing[],
): ExplorerListing {
  const name = path.includes("/") ? path.slice(path.lastIndexOf("/") + 1) : path;
  return { path, name, kind: "directory", children };
}

function file(path: string, fingerprint: string): ExplorerListing {
  const name = path.includes("/") ? path.slice(path.lastIndexOf("/") + 1) : path;
  return { path, name, kind: "file", fingerprint };
}

test("nested src/lib.rs sits deeper than src in the merged explorer", () => {
  const tree = mergeExplorerTrees(
    [dir("src", [file("src/lib.rs", "fn a() {}")])],
    [dir("src", [file("src/lib.rs", "fn a() {}")])],
  );
  const flat = flattenExplorer(tree);
  const src = flat.find((node) => node.path === "src");
  const lib = flat.find((node) => node.path === "src/lib.rs");
  assert.ok(src, "src folder is present");
  assert.ok(lib, "src/lib.rs is present");
  assert.equal(src.kind, "directory");
  assert.equal(lib.kind, "file");
  assert.ok(lib.depth > src.depth, "file is indented under its folder");
  assert.equal(src.depth, 0);
  assert.equal(lib.depth, 1);
});

test("a path only on one side is untracked with a U badge", () => {
  const tree = mergeExplorerTrees(
    [file("README.md", "hello")],
    [file("notes.md", "remote only")],
  );
  const flat = flattenExplorer(tree);
  const readme = flat.find((node) => node.path === "README.md");
  const notes = flat.find((node) => node.path === "notes.md");
  assert.equal(readme?.badge, "U");
  assert.equal(readme?.change, "untracked");
  assert.equal(notes?.badge, "U");
  assert.equal(notes?.change, "untracked");
});

test("a path whose contents differ is modified with an M badge", () => {
  const tree = mergeExplorerTrees(
    [dir("src", [file("src/lib.rs", "fn a() {}")])],
    [dir("src", [file("src/lib.rs", "fn b() {}")])],
  );
  const lib = flattenExplorer(tree).find((node) => node.path === "src/lib.rs");
  assert.equal(lib?.badge, "M");
  assert.equal(lib?.change, "modified");
});

test("listingsFromEntries marks content diffs as M when remote size is missing", () => {
  const local: ClaudeFileEntry[] = [
    {
      path: "src",
      name: "src",
      kind: "directory",
      side: "local",
      children: [
        {
          path: "src/lib.rs",
          name: "lib.rs",
          kind: "file",
          side: "local",
          size: 10,
          fingerprint: "fn a() {}",
        },
      ],
    },
  ];
  const remote: ClaudeFileEntry[] = [
    {
      path: "src",
      name: "src",
      kind: "directory",
      side: "remote",
      children: [
        {
          path: "src/lib.rs",
          name: "lib.rs",
          kind: "file",
          side: "remote",
          size: null,
          fingerprint: "fn b() {}",
        },
      ],
    },
  ];
  const tree = mergeExplorerTrees(listingsFromEntries(local), listingsFromEntries(remote));
  const lib = flattenExplorer(tree).find((node) => node.path === "src/lib.rs");
  assert.equal(lib?.badge, "M");
  assert.equal(lib?.change, "modified");
});

test("matching fingerprints on both sides stay unmarked", () => {
  const tree = mergeExplorerTrees([file("same.txt", "ok")], [file("same.txt", "ok")]);
  const node = flattenExplorer(tree).find((item) => item.path === "same.txt");
  assert.equal(node?.badge, "");
  assert.equal(node?.change, "unchanged");
});

test("Files markup is a single indented explorer, not a two-column wrap", async () => {
  const views = await readAllClaudeViews();
  const css = await readAllClaudeCss();
  assert.match(views, /mergeExplorerTrees/);
  assert.match(views, /listingsFromEntries/);
  assert.doesNotMatch(views, /lumio-claude-files-split/);
  assert.doesNotMatch(css, /\.lumio-claude-files-split/);
  assert.doesNotMatch(css, /\.lumio-claude-files li\s*\{[^}]*display:\s*flex/s);
  assert.match(css, /\.lumio-claude-file-row/);
  assert.match(css, /--depth/);
});
