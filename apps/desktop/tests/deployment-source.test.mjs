import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = await readFile(
  new URL("../src/components/OnboardingWizard.tsx", import.meta.url),
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

test("onboarding previews before executing the managed remote deployment", () => {
  const preview = source.indexOf('"preview_remote_deployment"');
  const provision = source.indexOf('invoke("provision_workspace_credential"');
  const probe = source.indexOf('invoke("probe_workspace_access"');
  const save = source.indexOf('invoke("save_project"');
  const execute = source.indexOf('invoke("execute_remote_deployment"');

  assert.ok(preview >= 0, "read-only deployment preview is missing");
  assert.ok(provision > preview, "credential provisioning must follow preview");
  assert.ok(save > provision, "project persistence must follow provisioning");
  assert.ok(execute > save, "remote writes must use the accepted, persisted project");
  assert.ok(
    probe > execute,
    "workspace access must be verified after deployment registers the root",
  );
  assert.match(source, /listen<DeployProgress>\(\s*"deploy:\/\/progress"/);
  assert.match(source, /invoke\("cancel_remote_deployment"/);
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
