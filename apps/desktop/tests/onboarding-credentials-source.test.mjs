import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = await readFile(
  new URL("../src/components/OnboardingWizard.tsx", import.meta.url),
  "utf8",
);
const rustSource = await readFile(
  new URL("../src-tauri/src/lib.rs", import.meta.url),
  "utf8",
);

test("onboarding provisions the stable project identity before saving", () => {
  const provision = source.indexOf('invoke("provision_workspace_credential"');
  const probe = source.indexOf('invoke("probe_workspace_access"');
  const save = source.indexOf('invoke("save_project"');

  assert.ok(provision >= 0, "credential provisioning command is missing");
  assert.ok(probe > provision, "workspace access must be probed after provisioning");
  assert.ok(save > probe, "project must not be saved before workspace acceptance");
  assert.match(source, /useState\(\(\) => crypto\.randomUUID\(\)\)/);
  assert.match(source, /workspaceId:\s*workspaceId/);
  assert.doesNotMatch(source, /workspaceId:\s*crypto\.randomUUID\(\)/);
});

test("onboarding collects secrets without logging and cleans failed setup", () => {
  assert.match(source, /type="password"/);
  assert.match(
    source,
    /invoke<CredentialRollbackStatus>\(\s*"cancel_workspace_provisioning"/,
  );
  assert.match(source, /credentialDeleted/);
  assert.match(source, /cleanup=\$\{codes\.join\(","\)\}/);
  assert.doesNotMatch(source, /console\.(?:log|warn|error|debug)/);
  assert.doesNotMatch(source, /invoke(?:<[^>]+>)?\("create_tunnel"/);
});

test("onboarding owns cancellation and rejects late invoke completion", () => {
  assert.match(source, /useRef\(0\)/);
  assert.match(source, /operationGeneration\.current/);
  assert.match(
    source,
    /invoke(?:<[^>]+>)?\(\s*"cancel_workspace_provisioning"/,
  );
  assert.match(source, /invoke\("delete_project"/);
  assert.match(source, /return \(\) =>/);
  assert.match(source, /disabled=\{saving\}/);
  assert.match(source, /credential cleanup did not delete/);
  assert.match(source, /if \([^)]*generation[^)]*!==[^)]*operationGeneration\.current/);
});

test("unmount rollback is observable and does not race a separate credential delete", () => {
  const cancel = source.indexOf('"cancel_workspace_provisioning"');
  const projectDelete = source.indexOf('invoke("delete_project"');

  assert.ok(cancel >= 0, "rollback must use the typed credential cancellation result");
  assert.ok(projectDelete > cancel, "project deletion must follow credential rollback");
  assert.match(source, /credentialDeleted/);
  assert.match(source, /pendingAgentDeletion/);
  assert.match(source, /workspace_credential_cleanup_status/);
  assert.match(source, /Agent credential deletion pending/);
  assert.match(source, /pendingUnmountCleanupFailure/);
  assert.doesNotMatch(source, /\.catch\(\s*\(\)\s*=>\s*undefined/);
  assert.doesNotMatch(source, /invoke\("delete_workspace_credential"/);
});

test("desktop production wiring shares the Keychain-backed provider with sync", () => {
  assert.match(
    rustSource,
    /let credential_state = credentials::CredentialState::production\(\)/,
  );
  assert.match(
    rustSource,
    /SyncState::with_credentials\(Arc::new\(credential_state\.clone\(\)\)\)/,
  );
  assert.doesNotMatch(rustSource, /\.manage\(sync::SyncState::new\(\)\)/);
  assert.match(rustSource, /credentials::probe_workspace_access/);
});
