import assert from "node:assert/strict";
import test from "node:test";

import { HELP_URL } from "./help.ts";

test("the help center lives on the BestCodex site", () => {
  assert.equal(HELP_URL, "https://bestcodex.app/help");
});
