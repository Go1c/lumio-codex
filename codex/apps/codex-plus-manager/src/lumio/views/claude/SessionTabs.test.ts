import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

async function readView(name: string): Promise<string> {
  return readFile(new URL(name, import.meta.url), "utf8");
}

test("SessionTabs copy, tablist, and title truncation follow the prototype", async () => {
  const source = await readView("SessionTabs.tsx");
  assert.match(source, /新对话/);
  assert.match(source, /这个对话正在跑，关掉就断了。/);
  assert.match(source, /还是关掉/);
  assert.match(source, /先留着/);
  assert.match(source, /role="tablist"/);
  assert.match(source, /import \{[^}]*\bclipWidth\b[^}]*\} from ["'][^"']*session-title\.ts["']/);
  assert.doesNotMatch(source, /function clipWidth/);
  assert.doesNotMatch(source, /\\u1100/);
  assert.doesNotMatch(source, /Claude Code ·/);
  assert.doesNotMatch(source, /\bagent\b/i);
  assert.doesNotMatch(source, /\btmux\b/i);
});

test("SessionTabs.css is a compact strip with sized icon shells and horizontal overflow", async () => {
  const css = await readView("SessionTabs.css");
  assert.match(css, /overflow-x:\s*auto/);
  assert.match(css, /\.glyph[\s\S]*?width:\s*14px/);
  assert.match(css, /\.glyph[\s\S]*?height:\s*14px/);
  assert.match(css, /\.x[\s\S]*?width:\s*18px/);
  assert.match(css, /\.x[\s\S]*?height:\s*18px/);
  assert.match(css, /width:\s*28px/);
  assert.match(css, /height:\s*28px/);
  assert.doesNotMatch(css, /\bagent\b/i);
  assert.doesNotMatch(css, /\btmux\b/i);
});
