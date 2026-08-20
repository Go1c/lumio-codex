import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

async function readExplorer(): Promise<string> {
  return readFile(new URL("FileExplorer.tsx", import.meta.url), "utf8");
}

async function readExplorerCss(): Promise<string> {
  return readFile(new URL("FileExplorer.css", import.meta.url), "utf8");
}

test("FileExplorer ships Orca finder copy as real JSX", async () => {
  const source = await readExplorer();

  for (const snippet of [
    "全部折叠",
    "刷新文件管理器",
    "更多",
    "只看有改动的",
    "全部展开",
    "在本机打开项目文件夹",
    "查找文件",
    "搜索",
    "Aa",
    "ab",
    ".*",
    "区分大小写",
    "全字匹配",
    "使用正则表达式",
    "名称",
    "内容",
    "要包含的文件",
    "要排除的文件",
    "输入要在文件中搜索的内容",
    "新文件",
    "新建文件夹",
    "复制路径",
    "⌘⌥C",
    "复制相对路径",
    "⌘⌥⇧C",
    "查看文件",
    "在内置浏览器中打开",
    "打开 Markdown 预览",
    "在 Finder 中显示",
    "重命名",
    "↵",
    "删除",
    "⌘Backspace, Delete",
    "已改",
    "新增",
    "冲突",
  ]) {
    assert.match(source, new RegExp(snippet.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")), snippet);
  }

  const copyCount = source.split("复制").length - 1;
  assert.ok(copyCount >= 4, `expected copy / copy-path / copy-relative / duplicate, got ${copyCount}`);
});

test("FileExplorer reuses explorer-filter, file-icons, and file-tree merge", async () => {
  const source = await readExplorer();
  assert.match(source, /from "\.\.\/\.\.\/claude\/explorer-filter\.ts"/);
  assert.match(source, /from "\.\.\/\.\.\/claude\/file-icons\.ts"/);
  assert.match(source, /filterExplorerNodes/);
  assert.match(source, /sortExplorerNodes/);
  assert.match(source, /fileIconKind/);
  assert.match(source, /mergeExplorerTrees/);
  assert.match(source, /listingsFromEntries/);
  assert.match(source, /flattenVisible/);
  assert.match(source, /previewClaudeFile/);
  assert.match(source, /refreshClaudeFiles/);
  assert.match(source, /preventDefault/);
  assert.match(source, /onContextMenu/);
  assert.match(source, /only:\s*"md"/);
});

test("content search previews local file text instead of fingerprints", async () => {
  const source = await readExplorer();
  assert.match(source, /previewClaudeFile/);
  assert.match(source, /side:\s*"local"/);
  assert.doesNotMatch(source, /map\.set\(entry\.path,\s*entry\.fingerprint\)/);
});

test("FileExplorer source and CSS never mention agent or tmux", async () => {
  const source = await readExplorer();
  const css = await readExplorerCss();
  assert.doesNotMatch(source, /\bagent\b/i);
  assert.doesNotMatch(source, /\btmux\b/i);
  assert.doesNotMatch(css, /\bagent\b/i);
  assert.doesNotMatch(css, /\btmux\b/i);
  assert.doesNotMatch(css, /\.d-fx-/);
});

test("FileExplorer rows hover, 1px selected stroke, and ellipsis", async () => {
  const css = await readExplorerCss();
  assert.match(css, /\.lumio-claude-file-row/);
  assert.match(css, /--depth/);
  assert.match(css, /\.lumio-claude-file-row:hover|\.lumio-claude-fx-row:hover/);
  assert.match(css, /inset 0 0 0 1px/);
  assert.match(css, /text-overflow:\s*ellipsis/);
});

test("FileExplorer context actions call local fs and ask before write", async () => {
  const source = await readExplorer();
  assert.match(source, /mutateClaudeLocalFile/);
  assert.match(source, /localFsErrorCopy/);
  for (const action of [
    "create-file",
    "create-folder",
    "duplicate",
    "rename",
    "delete",
    "reveal",
    "open-folder",
    "open-file",
  ]) {
    assert.match(source, new RegExp(action), action);
  }
  assert.match(source, /lumio-claude-fx-ask/);
  assert.match(source, /要删除「/);
  assert.match(source, /untitled-folder/);
  assert.match(source, /"Enter"/);
  assert.match(source, /"Delete"/);
  assert.match(source, /"Backspace"/);
  assert.match(source, /onBlankContext|onTreeContext/);
});

test("FileExplorer CSS styles the name overlay", async () => {
  const css = await readExplorerCss();
  assert.match(css, /\.lumio-claude-fx-ask/);
});
