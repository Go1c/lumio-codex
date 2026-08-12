import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const fileTreeSource = await readFile(
  new URL("../src/components/FileTree.tsx", import.meta.url),
  "utf8",
);
const workspaceSource = await readFile(
  new URL("../src/components/WorkspaceView.tsx", import.meta.url),
  "utf8",
);
const filesRsSource = await readFile(
  new URL("../src-tauri/src/files.rs", import.meta.url),
  "utf8",
);

test("FileTree uses projectId instead of absolute localRoot", () => {
  assert.match(fileTreeSource, /projectId\s*:\s*string/);
  assert.match(fileTreeSource, /browse_files",\s*\{\s*projectId\s*\}/);
  assert.match(
    fileTreeSource,
    /read_file",\s*\{\s*projectId,\s*relativePath:\s*path,\s*\}/s,
  );
  assert.match(
    fileTreeSource,
    /open_in_finder",\s*\{\s*projectId,\s*relativePath:/s,
  );
  assert.doesNotMatch(fileTreeSource, /localRoot/);
  assert.doesNotMatch(fileTreeSource, /baseDir/);
});

test("WorkspaceView passes projectId into FileTree", () => {
  assert.match(workspaceSource, /<FileTree\s+projectId=\{project\.id\}\s*\/>/);
  assert.doesNotMatch(workspaceSource, /FileTree\s+localRoot=/);
});

test("Tauri file commands accept project_id and relative_path only", () => {
  assert.match(filesRsSource, /pub fn browse_files\(project_id: String\)/);
  assert.match(
    filesRsSource,
    /pub fn read_file\(project_id: String, relative_path: String\)/,
  );
  assert.match(filesRsSource, /pub fn open_in_finder\(/);
  assert.match(filesRsSource, /project_id: String,/);
  assert.match(filesRsSource, /relative_path: Option<String>/);
  assert.match(filesRsSource, /fn local_root_for_project_id/);
  assert.doesNotMatch(
    filesRsSource,
    /pub fn browse_files\(local_root: String\)/,
  );
  assert.doesNotMatch(filesRsSource, /base_dir: String/);
});
