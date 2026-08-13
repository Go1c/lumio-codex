import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = await readFile(
  new URL("../src/components/ProjectWizard.tsx", import.meta.url),
  "utf8",
);
const deploySource = await readFile(
  new URL("../src-tauri/src/deploy.rs", import.meta.url),
  "utf8",
);
const tauriConfig = await readFile(
  new URL("../src-tauri/tauri.conf.json", import.meta.url),
  "utf8",
);

test("the wizard previews before executing the managed remote deployment", () => {
  const preview = source.indexOf("api.previewDeployment(");
  const provision = source.indexOf("api.provisionCredential(");
  const save = source.indexOf("api.saveProject(buildConfig(), password || undefined)", provision);
  const execute = source.indexOf("api.executeDeployment(");
  const probe = source.indexOf("api.probeWorkspaceAccess(");

  assert.ok(preview >= 0, "read-only deployment preview is missing");
  assert.ok(provision > preview, "credential provisioning must follow preview");
  assert.ok(save > provision, "project persistence must follow provisioning");
  assert.ok(execute > save, "remote writes must use the accepted, persisted project");
  assert.ok(
    probe > execute,
    "workspace access must be verified after deployment registers the root",
  );
  // A preview carrying blocking warnings must never be executable.
  assert.match(source, /preview\.warnings\.length > 0/);
  assert.match(source, /EVENTS\.deployProgress/);
  assert.match(source, /api\.cancelDeployment\(/);
});

test("users never provide deployment artifact paths", () => {
  assert.doesNotMatch(source, /serverArtifactPath|agentArtifactPath/);
  assert.doesNotMatch(deploySource, /pub\(crate\) server_artifact_path/);
  assert.doesNotMatch(deploySource, /pub\(crate\) agent_artifact_path/);
  assert.match(deploySource, /resource_dir\(\)/);
  assert.match(deploySource, /join\("remote"\)\.join\("linux-x86_64"\)/);
});

test("the app bundle carries both pinned remote executables", () => {
  const config = JSON.parse(tauriConfig);
  const resources = config.bundle.resources;
  assert.equal(
    resources["resources/remote/linux-x86_64/fns-server"],
    "remote/linux-x86_64/fns-server",
  );
  assert.equal(
    resources["resources/remote/linux-x86_64/fns-agent"],
    "remote/linux-x86_64/fns-agent",
  );
  assert.equal(
    resources["resources/remote/linux-x86_64/release-provenance.json"],
    "remote/linux-x86_64/release-provenance.json",
  );
});

test("remote services are supervised and deployment has no detached fallback", () => {
  assert.match(deploySource, /systemctl --user/);
  assert.match(deploySource, /ServiceManager::System/);
  assert.doesNotMatch(deploySource, /Command::new\("(?:nohup|screen|tmux)"\)/);
  assert.doesNotMatch(deploySource, /(?:nohup|disown) /);
});
