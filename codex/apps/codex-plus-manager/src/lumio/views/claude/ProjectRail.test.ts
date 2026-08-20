import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = await readFile(new URL("./ProjectRail.tsx", import.meta.url), "utf8");
const css = await readFile(new URL("./ProjectRail.css", import.meta.url), "utf8");

test("the rail has per-server new project and a footer to connect a server", () => {
  assert.match(source, /连接新服务器/);
  assert.match(source, /新建项目/);
});

test("the rail uses Wave 0 grouping helpers instead of inlined fold rules", () => {
  assert.match(source, /from "\.\.\/\.\.\/claude\/rail-groups\.ts"/);
  assert.match(source, /groupProjectsByHost/);
  assert.match(source, /isServerGroupOpen/);
  assert.match(source, /shouldShowServerShell/);
});

test("login copy is only on the server-meta block, never on project rows", () => {
  const marker = "project-row";
  const at = source.indexOf(marker);
  assert.ok(at >= 0, "source must comment-partition project rows");
  const serverPart = source.slice(0, at);
  const projectPart = source.slice(at);
  assert.match(serverPart, /已登录/);
  assert.match(serverPart, /未登录/);
  assert.doesNotMatch(projectPart, /已登录/);
  assert.doesNotMatch(projectPart, /未登录/);
});

test("user-visible rail copy never says agent or tmux", () => {
  assert.doesNotMatch(source, /\bagent\b/i);
  assert.doesNotMatch(source, /\btmux\b/i);
  assert.doesNotMatch(css, /\bagent\b/i);
  assert.doesNotMatch(css, /\btmux\b/i);
});

test("icon buttons wrap svgs with width/height or a sized class", () => {
  assert.match(source, /width=\{1[1-6]\}/);
  assert.match(source, /height=\{1[1-6]\}/);
  assert.match(css, /\.glyph/);
  assert.match(css, /svg\s*\{[^}]*width:\s*1[1-6]px/);
  assert.match(css, /svg\s*\{[^}]*height:\s*1[1-6]px/);
});

test("the rail is scheme D: no entitlement line, no orders slot, no prototype d-* classes", () => {
  assert.doesNotMatch(source, /ClaudeEntitlementLine/);
  assert.doesNotMatch(source, /\{ordersSlot\}/);
  assert.doesNotMatch(source, /\bd-rail|\bd-srv|\bd-proj|\bd-live\b/);
  assert.doesNotMatch(css, /\.d-rail|\.d-srv|\.d-proj|\.d-live/);
  assert.match(css, /\.lumio-claude-rail/);
  assert.match(css, /\.lumio-claude-srv/);
  assert.match(css, /\.lumio-claude-proj/);
});
