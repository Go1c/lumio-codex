import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const tauriConfUrl = new URL("../src-tauri/tauri.conf.json", import.meta.url);
const tauriConfSource = await readFile(tauriConfUrl, "utf8");
const tauriConf = JSON.parse(tauriConfSource);

test("production_csp_is_configured", () => {
  const csp = tauriConf?.app?.security?.csp;
  assert.equal(typeof csp, "string", "csp must be a string, not null");
  assert.ok(csp.trim().length > 0, "csp must be non-empty");

  // script-src must be present and must not allow wildcard sources.
  assert.match(csp, /script-src\s+'self'/);
  assert.doesNotMatch(
    csp,
    /script-src[^;]*\*/,
    "script-src must not contain wildcard *",
  );

  // Baseline directives expected for the desktop shell.
  assert.match(csp, /default-src\s+'self'/);
  assert.match(csp, /connect-src[^;]*\bipc:/);
  assert.equal(csp.includes("null"), false);
});
