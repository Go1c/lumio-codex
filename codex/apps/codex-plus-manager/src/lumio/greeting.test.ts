import assert from "node:assert/strict";
import test from "node:test";

import { greetingNameFromEmail } from "./greeting.ts";

test("the home greeting uses the capitalized email local part", () => {
  assert.equal(greetingNameFromEmail("mary@example.com"), "Mary");
});
