import assert from "node:assert/strict";
import test from "node:test";

import {
  groupProjectsByHost,
  isServerGroupOpen,
  shouldShowServerShell,
} from "./rail-groups.ts";
import type { ClaudeProject } from "./types.ts";

function project(id: string, host: string, user = "root"): ClaudeProject {
  return {
    id,
    name: id,
    host,
    user,
    port: 22,
    auth: "password",
    keyPath: null,
    hostAlias: null,
    remoteRoot: `~/bestcodex/${id}`,
    localRoot: `~/BestCodex/${id}`,
    createdAt: "2026-08-20T00:00:00.000Z",
  };
}

test("groupProjectsByHost buckets projects by machine", () => {
  const groups = groupProjectsByHost([
    project("my-project", "108.80.81.15"),
    project("docs", "108.80.81.15"),
    project("sandbox", "192.168.1.40", "cui"),
  ]);
  assert.equal(groups.length, 2);
  assert.equal(groups[0]?.host, "108.80.81.15");
  assert.equal(groups[0]?.user, "root");
  assert.deepEqual(
    groups[0]?.projects.map((item) => item.id),
    ["my-project", "docs"],
  );
  assert.equal(groups[1]?.host, "192.168.1.40");
  assert.equal(groups[1]?.user, "cui");
});

test("a single server does not show the group shell and stays open", () => {
  assert.equal(shouldShowServerShell(1), false);
  assert.equal(shouldShowServerShell(2), true);
  assert.equal(
    isServerGroupOpen({
      host: "108.80.81.15",
      serverCount: 1,
      online: false,
      holdsActiveProject: false,
      collapsed: true,
    }),
    true,
  );
});

test("offline servers default to collapsed", () => {
  assert.equal(
    isServerGroupOpen({
      host: "192.168.1.40",
      serverCount: 2,
      online: false,
      holdsActiveProject: false,
      collapsed: false,
    }),
    false,
  );
});

test("the group that holds the active project stays open even when collapsed", () => {
  assert.equal(
    isServerGroupOpen({
      host: "108.80.81.15",
      serverCount: 2,
      online: false,
      holdsActiveProject: true,
      collapsed: true,
    }),
    true,
  );
});

test("an online group is open until the user collapses it", () => {
  assert.equal(
    isServerGroupOpen({
      host: "108.80.81.15",
      serverCount: 2,
      online: true,
      holdsActiveProject: false,
      collapsed: false,
    }),
    true,
  );
  assert.equal(
    isServerGroupOpen({
      host: "108.80.81.15",
      serverCount: 2,
      online: true,
      holdsActiveProject: false,
      collapsed: true,
    }),
    false,
  );
});
