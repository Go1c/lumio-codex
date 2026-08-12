import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import test, { after } from "node:test";
import { build } from "esbuild";

const desktopDirectory = dirname(dirname(fileURLToPath(import.meta.url)));
const outputDirectory = await mkdtemp(join(desktopDirectory, ".fns-auth-test-"));
const outputFile = join(outputDirectory, "auth.mjs");

await build({
  entryPoints: [join(desktopDirectory, "src/auth.ts")],
  bundle: true,
  format: "esm",
  platform: "node",
  outfile: outputFile,
  logLevel: "silent",
});

const { accountConnectionFailureMessage, isAuthenticationFailure } =
  await import(pathToFileURL(outputFile).href);

after(async () => {
  await rm(outputDirectory, { recursive: true, force: true });
});

test("missing, expired, rejected and wrong-scope credentials all request reconnect", () => {
  for (const primary of [
    "auth_required",
    "authentication_rejected",
    "forbidden",
    "insecure_credential",
    "scope_mismatch",
    "client_type_mismatch",
    "credential_access",
  ]) {
    assert.equal(isAuthenticationFailure({ primary, cleanup: [] }), true, primary);
  }
  assert.equal(isAuthenticationFailure({ primary: "network" }), false);
});

test("account failures are translated into actionable messages", () => {
  assert.equal(
    accountConnectionFailureMessage({ primary: "authentication_rejected" }),
    "The username or password was not accepted.",
  );
  assert.match(
    accountConnectionFailureMessage({ primary: "credential_access" }),
    /Keychain/,
  );
  assert.match(
    accountConnectionFailureMessage({ primary: "workspace_identity_mismatch" }),
    /project settings/,
  );
});
