import assert from "node:assert/strict";
import test from "node:test";

import {
  CONTENT_SEARCH_HINT,
  compileExplorerGlobs,
  compileExplorerMatcher,
  filterExplorerNodes,
  sortExplorerNodes,
} from "./explorer-filter.ts";

const tree = [
  { path: "src", kind: "dir" as const },
  { path: "src/lib.rs", kind: "file" as const, text: "fn retry() {}" },
  { path: "README.md", kind: "file" as const, text: "hello world" },
  { path: "package.json", kind: "file" as const, text: '"name": "my-project"' },
  { path: ".gitignore", kind: "file" as const, text: "node_modules" },
];

test("content search hint copy is the prototype empty-state line", () => {
  assert.equal(CONTENT_SEARCH_HINT, "输入要在文件中搜索的内容");
});

test("sortExplorerNodes puts directories first and compares path segments", () => {
  const sorted = sortExplorerNodes([
    { path: "z.ts", kind: "file" },
    { path: "src/lib.rs", kind: "file" },
    { path: "src", kind: "dir" },
    { path: "README.md", kind: "file" },
  ]);
  assert.deepEqual(
    sorted.map((node) => node.path),
    ["src", "src/lib.rs", "README.md", "z.ts"],
  );
});

test("name mode filters basename and is case-insensitive by default", () => {
  const hits = filterExplorerNodes(tree, {
    mode: "name",
    query: "readme",
    flags: { case: false, word: false, regex: false },
  });
  assert.notEqual(hits, "bad");
  if (hits === "bad") return;
  assert.deepEqual(
    hits.map((node) => node.path),
    ["README.md"],
  );
});

test("name mode with case flag requires the same case", () => {
  const hits = filterExplorerNodes(tree, {
    mode: "name",
    query: "readme",
    flags: { case: true, word: false, regex: false },
  });
  assert.notEqual(hits, "bad");
  if (hits === "bad") return;
  assert.deepEqual(hits, []);
});

test("word match requires a whole word", () => {
  const flags = { case: false, word: true, regex: false };
  const hits = filterExplorerNodes(tree, { mode: "content", query: "retry", flags });
  assert.notEqual(hits, "bad");
  if (hits === "bad") return;
  assert.equal(hits[0]?.path, "src/lib.rs");
  const miss = filterExplorerNodes(tree, { mode: "content", query: "retr", flags });
  assert.notEqual(miss, "bad");
  if (miss === "bad") return;
  assert.deepEqual(miss, []);
});

test("content mode without a query returns empty so the caller can show the hint", () => {
  const hits = filterExplorerNodes(tree, {
    mode: "content",
    query: "",
    flags: { case: false, word: false, regex: false },
  });
  assert.deepEqual(hits, []);
});

test("content mode filters the text field", () => {
  const hits = filterExplorerNodes(tree, {
    mode: "content",
    query: "hello",
    flags: { case: false, word: false, regex: false },
  });
  assert.notEqual(hits, "bad");
  if (hits === "bad") return;
  assert.deepEqual(
    hits.map((node) => node.path),
    ["README.md"],
  );
});

test("illegal regex returns bad", () => {
  assert.equal(
    filterExplorerNodes(tree, {
      mode: "name",
      query: "(",
      flags: { case: false, word: false, regex: true },
    }),
    "bad",
  );
  assert.equal(compileExplorerMatcher("(", { case: false, word: false, regex: true }), "bad");
});

test("globs without a slash match any directory level", () => {
  const globs = compileExplorerGlobs("*.rs, package.json");
  assert.equal(globs.some((glob) => glob.test("src/lib.rs")), true);
  assert.equal(globs.some((glob) => glob.test("package.json")), true);
  assert.equal(globs.some((glob) => glob.test("README.md")), false);
});

test("content mode honors include and exclude globs", () => {
  const included = filterExplorerNodes(tree, {
    mode: "content",
    query: "fn retry",
    flags: { case: false, word: false, regex: false },
    include: "*.rs",
  });
  assert.notEqual(included, "bad");
  if (included === "bad") return;
  assert.deepEqual(
    included.map((node) => node.path),
    ["src/lib.rs"],
  );
  const excluded = filterExplorerNodes(tree, {
    mode: "content",
    query: "fn retry",
    flags: { case: false, word: false, regex: false },
    exclude: "*.rs",
  });
  assert.notEqual(excluded, "bad");
  if (excluded === "bad") return;
  assert.deepEqual(excluded, []);
});
