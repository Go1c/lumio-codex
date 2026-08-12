import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const appSource = await readFile(
  new URL("../src/App.tsx", import.meta.url),
  "utf8",
);
const workspaceSource = await readFile(
  new URL("../src/components/WorkspaceView.tsx", import.meta.url),
  "utf8",
);

test("saved projects carry the workspace identity into reconnect", () => {
  assert.match(appSource, /interface Project[\s\S]*?workspaceId:\s*string/);
  assert.match(workspaceSource, /interface Project[\s\S]*?workspaceId:\s*string/);
  assert.match(workspaceSource, /workspaceId:\s*project\.workspaceId/);
});

test("authentication failures render one actionable account state", () => {
  assert.match(appSource, /import \{ isAuthenticationFailure \} from "\.\/auth"/);
  assert.match(
    appSource,
    /failedProjects[\s\S]*?!isAuthenticationFailure\(startFailures\[project\.id\]\)/,
  );
  assert.match(workspaceSource, /const authenticationRequired\s*=/);
  assert.match(workspaceSource, /credentialRequired \|\|/);
  assert.match(workspaceSource, /Connect your account/);
  assert.match(
    workspaceSource,
    /!authenticationRequired\s*&&[\s\S]*?<div className="sync-errors">/,
  );
  assert.match(workspaceSource, /Sign in to resume file sync/);
});

test("account reconnect validates, reprovisions, probes, then starts real sync", () => {
  const reconnectStart = workspaceSource.indexOf(
    "async function reconnectAccount",
  );
  const reconnectEnd = workspaceSource.indexOf("\n  const tabs", reconnectStart);
  const reconnect = workspaceSource.slice(reconnectStart, reconnectEnd);

  assert.ok(reconnectStart >= 0, "account reconnect handler is missing");
  assert.ok(reconnectEnd > reconnectStart, "account reconnect handler is incomplete");
  const stop = reconnect.indexOf('invoke("stop_sync"');
  const reprovision = reconnect.indexOf(
    'invoke("reprovision_workspace_credential"',
  );
  const probe = reconnect.indexOf('invoke("probe_workspace_access"');
  const restart = reconnect.indexOf("onRetryStart()");

  assert.ok(stop >= 0, "running sync must be stopped before credential rotation");
  assert.ok(reprovision > stop, "credential rotation must follow an optional stop");
  assert.ok(probe > reprovision, "workspace access must be probed after rotation");
  assert.ok(restart > probe, "sync must start only after the probe succeeds");
  assert.match(reconnect, /if \(status\?\.running\)/);
  assert.match(reconnect, /projectId:\s*project\.id/);
  assert.match(reconnect, /sshHostAlias:\s*project\.sshHostAlias/);
  assert.match(reconnect, /workspaceId:\s*project\.workspaceId/);
});

test("startup reads the saved credential only once through the sync process", () => {
  const start = appSource.slice(
    appSource.indexOf("async function startProject"),
    appSource.indexOf("async function loadProjects"),
  );

  assert.match(start, /invoke\("start_sync"/);
  assert.doesNotMatch(start, /workspace_credential_status/);
  assert.match(start, /if \(isAuthenticationFailure\(error\)\)/);
  assert.match(start, /setCredentialRequired/);
});

test("reconnect is bounded to the active project and never retains a password", () => {
  assert.match(workspaceSource, /connectionInFlight\.current/);
  assert.match(workspaceSource, /connectionGeneration\.current/);
  assert.match(
    workspaceSource,
    /generation\s*!==\s*connectionGeneration\.current/,
  );
  assert.match(
    workspaceSource,
    /return \(\) =>\s*{\s*connectionGeneration\.current \+= 1;/,
  );
  assert.match(workspaceSource, /finally\s*{[\s\S]*?setPassword\(""\)/);
  assert.match(workspaceSource, /type="password"/);
  assert.match(workspaceSource, /required/);
  assert.match(workspaceSource, /disabled=\{connecting/);
  assert.match(workspaceSource, /accountConnectionFailureMessage/);
  assert.doesNotMatch(workspaceSource, /console\.(?:log|warn|error|debug)/);
});
