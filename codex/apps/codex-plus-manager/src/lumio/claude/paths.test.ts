import assert from "node:assert/strict";
import test from "node:test";

import { localProjectRoot, projectSlug, remoteProjectRoot } from "./paths.ts";

test("projects are preset under the login home, not a guessed /root or /home path", () => {
  assert.equal(remoteProjectRoot("root", "my-project"), "~/bestcodex/my-project");
  assert.equal(remoteProjectRoot("ubuntu", "api"), "~/bestcodex/api");
});

test("the local preset is ~/BestCodex/{name}", () => {
  assert.equal(localProjectRoot("my-project"), "~/BestCodex/my-project");
});

test("project slugs keep readable names and fall back to my-project", () => {
  assert.equal(projectSlug("docs site"), "docs-site");
  assert.equal(projectSlug("   "), "my-project");
});
