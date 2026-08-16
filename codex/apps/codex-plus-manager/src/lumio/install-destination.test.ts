import assert from "node:assert/strict";
import { test } from "node:test";

import { destinationOptions } from "./install-destination.ts";

test("windows offers the managed standard install next to a chosen directory", () => {
  const options = destinationOptions("windows");
  assert.equal(options.length, 2);
  assert.equal(options[0].id, "standard");
  assert.match(options[0].note ?? "", /系统设置/);
  assert.equal(options[1].id, "choose");
  assert.match(options[1].label, /选择/);
});

test("macos defaults to /Applications with a folder chooser", () => {
  const options = destinationOptions("macos");
  assert.equal(options.length, 2);
  assert.equal(options[0].id, "standard");
  assert.match(options[0].label, /Applications/);
  assert.equal(options[1].id, "choose");
});

test("unknown platforms fall back to the windows-shaped choice", () => {
  assert.equal(destinationOptions("")[0].id, "standard");
  assert.equal(destinationOptions("linux")[1].id, "choose");
});
