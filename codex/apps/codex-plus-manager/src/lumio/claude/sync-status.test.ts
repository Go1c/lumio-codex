import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { readAllClaudeViews } from "./read-claude-views.ts";
import {
  SYNC_REINSTALL_LABEL,
  SYNC_RELAUNCH_LABEL,
  projectsToResume,
  reconcileSyncWithRemote,
  resumeSavedProjects,
  syncNeedsRecovery,
  workspaceStatusAppearance,
  workspaceStatusCopy,
} from "./sync-status.ts";
import type { ClaudeProject, ClaudeServerStatus, ClaudeSyncStatus } from "./types.ts";

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

function running(): ClaudeSyncStatus {
  return { state: "running", filesDone: 2, filesTotal: 8, errorCode: null, conflicts: 0 };
}

function remoteStatus(syncRunning: boolean): ClaudeServerStatus {
  return {
    projectId: "p-docs",
    capturedAt: "1",
    ok: true,
    services: {
      items: [
        {
          key: "sync",
          displayName: "同步组件",
          running: syncRunning,
          processCount: syncRunning ? 1 : 0,
          cpuPercent: 0,
          memoryRssBytes: 0,
        },
        {
          key: "workspace",
          displayName: "远端服务",
          running: true,
          processCount: 1,
          cpuPercent: 0,
          memoryRssBytes: 79 * 1024 * 1024,
        },
      ],
    },
  };
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
  assert.equal(
    workspaceStatusCopy({
      state: "fail",
      filesDone: 0,
      filesTotal: 0,
      errorCode: "SYNC_ENGINE_UNAVAILABLE",
      conflicts: 0,
    }),
    "同步不可用",
  );
  assert.match(workspaceStatusCopy(running()), /同步运行中|正在同步/);
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

test("idle, fail, and a stopped remote sync component are red warnings, not 同步运行中", () => {
  assert.equal(workspaceStatusAppearance(null).tone, "bad");
  assert.equal(workspaceStatusAppearance(idle()).tone, "bad");
  assert.match(workspaceStatusAppearance(idle()).copy, /同步未运行/);

  const failed = workspaceStatusAppearance({
    state: "fail",
    filesDone: 0,
    filesTotal: 0,
    errorCode: "SYNC_ENGINE_UNAVAILABLE",
    conflicts: 0,
  });
  assert.equal(failed.tone, "bad");
  assert.equal(failed.copy, "同步不可用");
  assert.ok(failed.copy.length <= 6);
  assert.doesNotMatch(failed.copy, /这个版本|暂时拉不了|更新或重装|BestCodex/);

  const remoteStart = workspaceStatusAppearance({
    state: "fail",
    filesDone: 0,
    filesTotal: 0,
    errorCode: "SYNC_REMOTE_START_FAILED",
    conflicts: 0,
  });
  assert.equal(remoteStart.copy, "同步未运行");
  assert.ok(remoteStart.copy.length <= 6);
  assert.doesNotMatch(remoteStart.copy, /这个版本|暂时拉不了|更新或重装|BestCodex/);

  const remoteDown = workspaceStatusAppearance(running(), remoteStatus(false));
  assert.equal(remoteDown.tone, "bad");
  assert.equal(remoteDown.copy, "同步未运行");

  const remoteUp = workspaceStatusAppearance(running(), remoteStatus(true));
  assert.match(remoteUp.copy, /同步运行中/);
  assert.notEqual(remoteUp.tone, "bad");
});

test("a local running flag is not kept when the remote sync component is down", () => {
  const next = reconcileSyncWithRemote(running(), remoteStatus(false));
  assert.equal(next?.state, "fail");
  assert.equal(next?.errorCode, "SYNC_REMOTE_NOT_RUNNING");
  assert.equal(reconcileSyncWithRemote(running(), remoteStatus(true))?.state, "running");
  assert.equal(reconcileSyncWithRemote(running(), null)?.state, "running");
});

test("hydrate and open invoke resume, not only SSH list files", async () => {
  const session = await readFile(new URL("./session.ts", import.meta.url), "utf8");
  const views = await readAllClaudeViews();
  const api = await readFile(new URL("./api.ts", import.meta.url), "utf8");
  assert.match(session, /resumeSavedProjects/);
  assert.match(session, /hydrateClaudeWorkspace[\s\S]*resumeSavedProjects/s);
  assert.match(session, /resumeClaudeSync/);
  assert.match(views, /resumeClaudeSync/);
  assert.match(views, /workspaceStatusAppearance/);
  assert.match(session, /applyRemoteSyncHealth|reconcileSyncWithRemote/);
  assert.match(session, /running === false|payload\.running === false/);
  assert.doesNotMatch(views, /本机目录已就绪/);
  assert.match(api, /lumio_claude_resume_sync/);
  assert.match(api, /SYNC_REMOTE_NOT_RUNNING/);
  assert.doesNotMatch(session, /listClaudeFiles\(\{[\s\S]*\}\);\s*dispatchClaude\(\{\s*type: "project-sync-updated"/);
});

test("failed or stopped sync needs a recovery action, not only 刷新", () => {
  assert.equal(SYNC_RELAUNCH_LABEL, "重新拉起");
  assert.equal(SYNC_REINSTALL_LABEL, "重装");
  assert.equal(
    syncNeedsRecovery({
      state: "fail",
      filesDone: 0,
      filesTotal: 0,
      errorCode: "SYNC_REMOTE_NOT_RUNNING",
      conflicts: 0,
    }),
    true,
  );
  assert.equal(syncNeedsRecovery(running(), remoteStatus(false)), true);
  assert.equal(syncNeedsRecovery(running(), remoteStatus(true)), false);
});

test("left rail, status bar, and server drawer expose 重新拉起", async () => {
  const rail = await readFile(new URL("../views/claude/ProjectRail.tsx", import.meta.url), "utf8");
  const bar = await readFile(new URL("../views/claude/StatusBar.tsx", import.meta.url), "utf8");
  const drawer = await readFile(new URL("../views/claude/StatusDrawer.tsx", import.meta.url), "utf8");
  for (const [name, source] of [
    ["ProjectRail", rail],
    ["StatusBar", bar],
    ["StatusDrawer", drawer],
  ] as const) {
    assert.match(
      source,
      /SYNC_RELAUNCH_LABEL|重新拉起/,
      `${name} must offer 重新拉起 when sync is down`,
    );
  }
  assert.match(drawer, /SYNC_REINSTALL_LABEL|重装/);
  assert.match(drawer, /已经装过同步组件/);
});
