import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import type { ClaudeChatSession } from "../../claude/types.ts";
import {
  claudeVersionLoginCopy,
  collectSessions,
  conflictFlagCopy,
  conversationCountCopy,
  hostResourceCopy,
  readyStatusCopy,
  updateNudgeCopy,
} from "./status-copy.ts";

async function readOwn(name: string): Promise<string> {
  return readFile(new URL(name, import.meta.url), "utf8");
}

function session(partial: Partial<ClaudeChatSession> = {}): ClaudeChatSession {
  return {
    id: "s1",
    projectId: "p1",
    title: null,
    titleLocked: false,
    running: false,
    ...partial,
  };
}

test("conversation count is 对话 N, or 对话 N · M 在跑 when any session is running", () => {
  assert.equal(conversationCountCopy(2, 1), "对话 2 · 1 在跑");
  assert.equal(conversationCountCopy(2, 0), "对话 2");
  assert.equal(conversationCountCopy(0, 0), "对话 0");
});

test("conversation counts flatten sessions from every project, not only the active one", () => {
  const all = collectSessions({
    alpha: [session({ id: "a", projectId: "alpha", running: true })],
    beta: [session({ id: "b", projectId: "beta" }), session({ id: "c", projectId: "beta", running: true })],
  });
  assert.equal(all.length, 3);
  assert.equal(conversationCountCopy(all.length, all.filter((item) => item.running).length), "对话 3 · 2 在跑");
});

test("ready copy covers init, resume, offline, and the everyday ready state", () => {
  assert.equal(readyStatusCopy("init", null).label, "正在准备");
  assert.equal(readyStatusCopy("resume", null).label, "正在连接");
  assert.equal(readyStatusCopy("offline", null).tone, "bad");
  assert.equal(readyStatusCopy("offline", null).label, "离线");
  assert.equal(readyStatusCopy("ready", { state: "offline", filesDone: 0, filesTotal: 0, errorCode: null, conflicts: 0 }).label, "离线");
  assert.equal(readyStatusCopy("ready", null).label, "已就绪");
  assert.equal(readyStatusCopy("ready", null).tone, "ok");
});

test("Claude version and login stay on the server line, with an update nudge when a newer version exists", () => {
  assert.equal(
    claudeVersionLoginCopy({ version: "2.1.228", latest: "2.1.230" }, { phase: "logged-in" }),
    "Claude 2.1.228 · 已登录",
  );
  assert.equal(claudeVersionLoginCopy({ version: null, latest: null }, { phase: "logged-out" }), "Claude · 未登录");
  assert.equal(claudeVersionLoginCopy({ version: "2.1.228", latest: null }, { phase: "expired" }), "Claude 2.1.228 · 登录已过期");
  assert.equal(updateNudgeCopy({ version: "2.1.228", latest: "2.1.230" }), "有新版 2.1.230 · 升级");
  assert.equal(updateNudgeCopy({ version: "2.1.228", latest: "2.1.228" }), null);
  assert.equal(updateNudgeCopy({ version: "2.1.228", latest: null }), null);
});

test("CPU and memory copy only exists when a host snapshot is present", () => {
  assert.equal(hostResourceCopy(null), null);
  assert.equal(
    hostResourceCopy({ cpu: { usagePercent: 12.4 }, memory: { usedPercent: 48 } }),
    "CPU 12% · 内存 48%",
  );
});

test("conflict badge is a count, not a blocking message", () => {
  assert.equal(conflictFlagCopy(0), null);
  assert.equal(conflictFlagCopy(2), "冲突 2");
});

test("StatusBar.tsx renders 对话 / 在跑, dispatches the drawer, and never says agent or tmux", async () => {
  const source = await readOwn("StatusBar.tsx");
  assert.match(source, /对话/);
  assert.match(source, /在跑/);
  assert.match(source, /conversationCountCopy/);
  assert.match(source, /collectSessions/);
  assert.match(source, /sessionsByProject/);
  assert.match(source, /set-status-drawer/);
  assert.match(source, /workspaceStatusCopy/);
  assert.match(source, /user\}@\$\{active\.host/);
  assert.doesNotMatch(source, /\bagent\b/i);
  assert.doesNotMatch(source, /\btmux\b/i);
});

test("StatusBar.css is a 26px one-line bar", async () => {
  const css = await readOwn("StatusBar.css");
  assert.match(css, /26px/);
  assert.match(css, /11px/);
  assert.doesNotMatch(css, /\bagent\b/i);
  assert.doesNotMatch(css, /\btmux\b/i);
});

test("StatusBar copy matches the scheme D one-line status, not the launcher disclaimer", async () => {
  const source = await readOwn("StatusBar.tsx");
  assert.match(source, /● \{ready\.label\}/);
  assert.match(source, /claudeVersionLoginCopy/);
  assert.match(source, /hostResourceCopy/);
  assert.match(source, /conversationCountCopy/);
  assert.doesNotMatch(source, /官方应用需单独安装/);
  assert.doesNotMatch(source, /BestCodex/);
});
