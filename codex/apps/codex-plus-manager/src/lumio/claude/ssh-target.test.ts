import assert from "node:assert/strict";
import test from "node:test";

import { parseSshTarget } from "./ssh-target.ts";

test("pasting ssh root@host splits out the user and host", () => {
  assert.deepEqual(parseSshTarget("ssh root@43.156.20.8"), {
    host: "43.156.20.8",
    user: "root",
    port: null,
  });
});

test("a bare user@host line is recognised", () => {
  assert.deepEqual(parseSshTarget("ubuntu@example.com"), {
    host: "example.com",
    user: "ubuntu",
    port: null,
  });
});

test("ssh -p before or after the target keeps the port", () => {
  assert.deepEqual(parseSshTarget("ssh root@1.2.3.4 -p 2222"), {
    host: "1.2.3.4",
    user: "root",
    port: 2222,
  });
  assert.deepEqual(parseSshTarget("ssh -p 2222 ubuntu@example.com"), {
    host: "example.com",
    user: "ubuntu",
    port: 2222,
  });
});

test("a plain host is accepted without inventing a user", () => {
  assert.deepEqual(parseSshTarget("43.156.20.8"), {
    host: "43.156.20.8",
    user: null,
    port: null,
  });
});

test("empty or multi-line text is not a target", () => {
  assert.equal(parseSshTarget(""), null);
  assert.equal(parseSshTarget("   "), null);
  assert.equal(parseSshTarget("ssh root@a\nssh root@b"), null);
});
