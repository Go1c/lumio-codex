import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { DEFAULT_SESSION_TITLE } from "../../claude/session-title.ts";
import type { ClaudeChatSession, ClaudeProject, ClaudeState } from "../../claude/types.ts";
import { liveSessionRows, sessionRowStatus, sessionTitleCopy } from "./status-copy.ts";

async function readOwn(name: string): Promise<string> {
  return readFile(new URL(name, import.meta.url), "utf8");
}

function project(id: string, name: string): ClaudeProject {
  return {
    id,
    name,
    host: "108.80.81.15",
    user: "root",
    port: 22,
    auth: "password",
    keyPath: null,
    hostAlias: null,
    remoteRoot: `~/bestcodex/${name}`,
    localRoot: `~/BestCodex/${name}`,
    createdAt: "2026-08-20T00:00:00.000Z",
  };
}

function session(partial: Partial<ClaudeChatSession> & Pick<ClaudeChatSession, "id" | "projectId">): ClaudeChatSession {
  return {
    title: null,
    titleLocked: false,
    running: false,
    ...partial,
  };
}

test("live session rows include every project and use 新对话 until the title is locked", () => {
  const rows = liveSessionRows(
    [project("p-my", "my-project"), project("p-docs", "docs-site")],
    {
      "p-my": [
        session({ id: "s1", projectId: "p-my", title: "帮我把 sync.ts 里的重试逻辑…", titleLocked: true }),
        session({ id: "s2", projectId: "p-my", running: true }),
      ],
      "p-docs": [session({ id: "s3", projectId: "p-docs", title: "改一下部署说明的措辞", titleLocked: true })],
    },
  );
  assert.equal(rows.length, 3);
  assert.deepEqual(
    rows.map((row) => row.projectName),
    ["my-project", "my-project", "docs-site"],
  );
  assert.equal(sessionTitleCopy(rows[0]!.session), "帮我把 sync.ts 里的重试逻辑…");
  assert.equal(sessionTitleCopy(rows[1]!.session), DEFAULT_SESSION_TITLE);
  assert.equal(sessionTitleCopy(rows[1]!.session), "新对话");
});

test("session row status is 正在跑, 当前, or 后台", () => {
  const running = session({ id: "s2", projectId: "p-my", running: true });
  const current = session({ id: "s1", projectId: "p-my" });
  const other = session({ id: "s3", projectId: "p-docs" });
  assert.equal(sessionRowStatus(running, { activeProjectId: "p-my", activeSessionId: "s1" }), "正在跑");
  assert.equal(sessionRowStatus(current, { activeProjectId: "p-my", activeSessionId: "s1" }), "当前");
  assert.equal(sessionRowStatus(other, { activeProjectId: "p-my", activeSessionId: "s1" }), "后台");
});

test("StatusDrawer.tsx keeps ServerStatusPane and SessionsPane, lists store sessions, and imports ConflictsPane", async () => {
  const source = await readOwn("StatusDrawer.tsx");
  assert.match(source, /export function ServerStatusPane/);
  assert.match(source, /export function SessionsPane/);
  assert.match(source, /export function StatusDrawer/);
  assert.match(source, /from "\.\/ConflictsPane\.tsx"/);
  assert.match(source, /<ConflictsPane/);
  assert.match(source, /挂载点/);
  assert.match(source, /sessionsByProject|liveSessionRows/);
  assert.match(source, />服务器状态</);
  assert.match(source, />对话状态</);
  assert.match(source, />冲突</);
  assert.match(source, /收起/);
  assert.match(source, /set-status-drawer/);
  assert.match(source, /serviceDisplayName/);
  assert.doesNotMatch(source, /\bagent\b/i);
  assert.doesNotMatch(source, /\btmux\b/i);
});

test("StatusDrawer.css raises a drawer above the 26px bar", async () => {
  const css = await readOwn("StatusDrawer.css");
  assert.match(css, /bottom:\s*26px/);
  assert.match(css, /62%/);
  assert.doesNotMatch(css, /\bagent\b/i);
  assert.doesNotMatch(css, /\btmux\b/i);
});

test("status-copy helpers do not depend on ClaudeState persistence fields", () => {
  const state = {
    sessionsByProject: {} as ClaudeState["sessionsByProject"],
  };
  assert.deepEqual(liveSessionRows([], state.sessionsByProject), []);
});
