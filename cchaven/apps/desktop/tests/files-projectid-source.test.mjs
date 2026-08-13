import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const explorerSource = await readFile(
  new URL("../src/components/FilesExplorer.tsx", import.meta.url),
  "utf8",
);
const workspaceSource = await readFile(
  new URL("../src/components/Workspace.tsx", import.meta.url),
  "utf8",
);
const apiSource = await readFile(
  new URL("../src/lib/api.ts", import.meta.url),
  "utf8",
);
const filesRsSource = await readFile(
  new URL("../src-tauri/src/files.rs", import.meta.url),
  "utf8",
);

test("the explorer addresses files by projectId, never by absolute root", () => {
  assert.match(explorerSource, /projectId\s*:\s*string/);
  assert.doesNotMatch(explorerSource, /localRoot/);
  assert.doesNotMatch(explorerSource, /baseDir/);
});

test("the workspace passes projectId into the explorer", () => {
  assert.match(workspaceSource, /<FilesExplorer[\s\S]{0,200}projectId=\{project\.id\}/);
});

test("every file command carries projectId and a relative path", () => {
  for (const command of [
    "list_files",
    "read_file",
    "create_entry",
    "rename_entry",
    "delete_entry",
    "undo_delete",
    "reveal_entry",
    "open_entry",
  ]) {
    assert.match(apiSource, new RegExp(`"${command}"[^\\n]*projectId`));
  }
});

test("the Rust side resolves the root from persisted config", () => {
  assert.match(filesRsSource, /pub fn local_root_for_project_id/);
  assert.match(filesRsSource, /pub fn project_local_root_for/);
  // Confinement is enforced centrally, not per call site.
  assert.match(filesRsSource, /pub fn resolve_project_path/);
  assert.match(filesRsSource, /fn ensure_under_root/);
  assert.doesNotMatch(filesRsSource, /base_dir: String/);
  assert.doesNotMatch(filesRsSource, /pub fn browse_files\(local_root: String\)/);
});
