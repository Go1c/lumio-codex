import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = await readFile(
  new URL("../src/components/ProjectWizard.tsx", import.meta.url),
  "utf8",
);
const apiSource = await readFile(
  new URL("../src/lib/api.ts", import.meta.url),
  "utf8",
);
const rustSource = await readFile(
  new URL("../src-tauri/src/lib.rs", import.meta.url),
  "utf8",
);

test("the wizard deploys a stable project identity before probing access", () => {
  const provision = source.indexOf("api.provisionCredential(");
  const save = source.indexOf("api.saveProject(buildConfig(), password || undefined)", provision);
  const execute = source.indexOf("api.executeDeployment(");
  const probe = source.indexOf("api.probeWorkspaceAccess(");

  assert.ok(provision >= 0, "credential provisioning is missing");
  assert.ok(save > provision, "the project must be saved after provisioning");
  assert.ok(execute > save, "deployment must use the persisted project identity");
  assert.ok(
    probe > execute,
    "workspace access must be probed after deployment registers the root",
  );
  // The project id is minted once and reused, never regenerated mid-flow.
  assert.match(source, /useRef\(project\?\.id \?\? crypto\.randomUUID\(\)\)/);
  assert.match(source, /workspaceId:\s*config\.workspaceId/);
});

test("the wizard collects secrets without logging and rolls back failed setup", () => {
  assert.match(source, /type="password"/);
  assert.match(apiSource, /"cancel_workspace_provisioning"/);
  assert.match(source, /credentialDeleted/);
  assert.doesNotMatch(source, /console\.(?:log|warn|error|debug)/);
  assert.doesNotMatch(source, /api\.previewDeployment[\s\S]{0,40}create_tunnel/);
});

test("rollback is observable and ordered after the credential is gone", () => {
  const cancel = source.indexOf("api.cancelProvisioning(");
  const projectDelete = source.indexOf("api.deleteProject(projectIdRef.current)");

  assert.ok(cancel >= 0, "rollback must use the typed credential cancellation result");
  assert.ok(projectDelete > cancel, "project deletion must follow credential rollback");
  assert.match(source, /credentialCleanupStatus/);
  assert.match(source, /pendingAgentDeletion/);
  assert.match(source, /deploy\.cleanupAgentPending/);
  // Failures are reported, never swallowed: a half-provisioned server is
  // exactly what the user needs to hear about.
  assert.doesNotMatch(source, /rollback\(\)\.catch\(\s*\(\)\s*=>\s*undefined/);
  assert.doesNotMatch(source, /"delete_workspace_credential"/);
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
