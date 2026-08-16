import assert from "node:assert/strict";
import test from "node:test";

import { localProjectRoot, projectSlug, remoteProjectRoot } from "./paths.ts";

test("root projects are preset under /root/bestcodex/{name}", () => {
  assert.equal(remoteProjectRoot("root", "my-project"), "/root/bestcodex/my-project");
});

test("non-root users are preset under /home/{user}/bestcodex/{name}", () => {
  assert.equal(remoteProjectRoot("ubuntu", "api"), "/home/ubuntu/bestcodex/api");
});

test("the local preset is ~/BestCodex/{name}", () => {
  assert.equal(localProjectRoot("my-project"), "~/BestCodex/my-project");
});

test("project slugs keep readable names and fall back to my-project", () => {
  assert.equal(projectSlug("docs site"), "docs-site");
  assert.equal(projectSlug("   "), "my-project");
});
