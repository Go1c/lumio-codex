import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { readAllClaudeViews } from "./read-claude-views.ts";
import {
  projectsToResume,
  resumeSavedProjects,
  workspaceStatusCopy,
} from "./sync-status.ts";
import type { ClaudeProject, ClaudeSyncStatus } from "./types.ts";

function project(id = "p-docs"): ClaudeProject {
  return {
    id,
    name: "docs",
    host: "108.80.81.15",
    user: "root",
    port: 1080,
    auth: "password",
    keyPath: null,
    hostAlias: null,
    remoteRoot: "~/bestcodex/docs",
    localRoot: "~/BestCodex/docs",
    createdAt: "2026-08-20T00:00:00.000Z",
  };
}

function idle(): ClaudeSyncStatus {
  return { state: "idle", filesDone: 0, filesTotal: 0, errorCode: null, conflicts: 0 };
}

test("saved connected projects resume the official sync engine", async () => {
  const called: string[] = [];
  const saved = project();
  await resumeSavedProjects([saved], saved.id, async (id) => {
    called.push(id);
  });
  assert.deepEqual(called, [saved.id]);
  assert.deepEqual(projectsToResume([saved], null).map((item) => item.id), [saved.id]);
});

test("engine not running is not 本机目录已就绪", () => {
  assert.notEqual(workspaceStatusCopy(null), "本机目录已就绪");
  assert.notEqual(workspaceStatusCopy(idle()), "本机目录已就绪");
  assert.match(workspaceStatusCopy(null), /同步未运行/);
  assert.match(
    workspaceStatusCopy({
      state: "fail",
      filesDone: 0,
      filesTotal: 0,
      errorCode: "SYNC_ENGINE_UNAVAILABLE",
      conflicts: 0,
    }),
    /同步组件/,
  );
  assert.match(
    workspaceStatusCopy({
      state: "running",
      filesDone: 2,
      filesTotal: 8,
      errorCode: null,
      conflicts: 0,
    }),
    /同步运行中|正在同步/,
  );
  assert.match(
    workspaceStatusCopy({
      state: "conflicts",
      filesDone: 4,
      filesTotal: 4,
      errorCode: null,
      conflicts: 3,
    }),
    /3 个冲突/,
  );
});

test("hydrate and open invoke resume, not only SSH list files", async () => {
  const session = await readFile(new URL("./session.ts", import.meta.url), "utf8");
  const views = await readAllClaudeViews();
  const api = await readFile(new URL("./api.ts", import.meta.url), "utf8");
  assert.match(session, /resumeSavedProjects/);
  assert.match(session, /hydrateClaudeWorkspace[\s\S]*resumeSavedProjects/s);
  assert.match(session, /resumeClaudeSync/);
  assert.match(views, /resumeClaudeSync/);
  assert.match(views, /workspaceStatusCopy/);
  assert.doesNotMatch(views, /本机目录已就绪/);
  assert.match(api, /lumio_claude_resume_sync/);
  assert.doesNotMatch(session, /listClaudeFiles\(\{[\s\S]*\}\);\s*dispatchClaude\(\{\s*type: "project-sync-updated"/);
});
