import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const scripts = new URL("../../../scripts/", import.meta.url);
const [resolver, verifier, acceptance, release, stage, prepare, provenanceVerifier, linuxAgentBuilder, quitAcceptance, packageSource] = await Promise.all([
  readFile(new URL("resolve-macos-signing-identity.sh", scripts), "utf8"),
  readFile(new URL("verify-macos-arm64-bundle.sh", scripts), "utf8"),
  readFile(new URL("build-macos-arm64-acceptance.sh", scripts), "utf8"),
  readFile(new URL("build-macos-arm64-release.sh", scripts), "utf8"),
  readFile(new URL("stage-macos-arm64-sidecar.sh", scripts), "utf8"),
  readFile(new URL("prepare-remote-linux-x86_64-release.sh", scripts), "utf8"),
  readFile(new URL("verify-remote-linux-x86_64-provenance.sh", scripts), "utf8"),
  readFile(new URL("build-linux-x86_64-agent-release.sh", scripts), "utf8"),
  readFile(new URL("accept-macos-appleevent-quit.sh", scripts), "utf8"),
  readFile(new URL("../package.json", import.meta.url), "utf8"),
]);

test("macOS builds require one real signing identity without repository credentials", () => {
  assert.match(resolver, /APPLE_SIGNING_IDENTITY/);
  assert.match(resolver, /multiple valid Apple Development identities found/);
  assert.match(resolver, /ad-hoc signing is not allowed/);
  assert.doesNotMatch(resolver, /[0-9A-F]{40}/);
  assert.doesNotMatch(resolver, /Apple Development: [^/\n]+@/);

  for (const source of [acceptance, release]) {
    const resolve = source.indexOf("resolve-macos-signing-identity.sh");
    const build = source.indexOf("tauri build");
    const verify = source.indexOf("verify-macos-arm64-bundle.sh");
    assert.ok(resolve >= 0, "signing identity resolution is missing");
    assert.ok(build > resolve, "Tauri build must follow identity resolution");
    assert.ok(verify > build, "bundle verification must follow the build");
  }
});

test("macOS bundle verification rejects unstable identities and inspects DMG contents", () => {
  assert.match(verifier, /Signature=adhoc/);
  assert.match(verifier, /TeamIdentifier/);
  assert.match(verifier, /designated => cdhash/);
  assert.match(verifier, /identifier \\\"\$expected_identifier\\\"/);
  assert.match(verifier, /codesign --verify --deep --strict/);
  assert.match(verifier, /lipo .* -verify_arch/);
  assert.match(verifier, /hdiutil attach .* -readonly -nobrowse/);
  assert.match(verifier, /hdiutil detach/);
  assert.match(verifier, /verify_app "\$1" "\$outer_team"/);
  assert.match(verifier, /Contents\/Resources\/remote\/linux-x86_64\/fns-server/);
  assert.match(verifier, /Contents\/Resources\/remote\/linux-x86_64\/fns-agent/);
  assert.match(verifier, /Contents\/Resources\/remote\/linux-x86_64\/release-provenance\.json/);
  assert.match(verifier, /cmp -s "\$expected_source" "\$resource"/);
  assert.match(verifier, /verify-remote-linux-x86_64-provenance\.sh/);
});

test("macOS builders isolate outputs and select one exact release DMG", () => {
  assert.match(
    acceptance,
    /CARGO_TARGET_DIR="\$repo_root\/target\/macos-arm64-acceptance"/,
  );
  assert.match(
    release,
    /CARGO_TARGET_DIR="\$repo_root\/target\/macos-arm64-release"/,
  );
  assert.match(
    acceptance,
    /app="\$CARGO_TARGET_DIR\/\$target\/debug\/bundle\/macos\/FNS Workspace\.app"/,
  );
  assert.match(
    release,
    /app="\$CARGO_TARGET_DIR\/\$target\/release\/bundle\/macos\/FNS Workspace\.app"/,
  );
  assert.match(release, /FNS Workspace_\$\{version\}_aarch64\.dmg/);
  assert.doesNotMatch(release, /for dmg in/);
});

test("macOS staging pins bundled Linux resources to release assets", () => {
  assert.match(
    stage,
    /FNS_REMOTE_LINUX_X86_64_PROVENANCE/,
  );
  for (const source of [acceptance, release]) {
    assert.match(source, /prepare-remote-linux-x86_64-release\.sh/);
    assert.match(source, /target\/release-assets\/linux-x86_64\/fns-server/);
    assert.match(source, /target\/release-assets\/linux-x86_64\/fns-agent/);
    assert.match(source, /release-provenance\.json/);
  }
});

test("release provenance is tied to clean final commits and artifact hashes", () => {
  assert.match(linuxAgentBuilder, /status --porcelain=v1/);
  assert.match(linuxAgentBuilder, /x86_64-unknown-linux-gnu/);
  assert.match(linuxAgentBuilder, /sourceCommit/);
  assert.match(linuxAgentBuilder, /sha256/);
  assert.match(prepare, /FNS_AGENT_LINUX_X86_64_BUILD_PROVENANCE/);
  assert.match(prepare, /go build -buildvcs=true -trimpath/);
  assert.match(prepare, /clientCommit/);
  assert.match(prepare, /serverCommit/);
  assert.match(prepare, /buildTimestamp/);
  assert.match(prepare, /buildCommand/);
  assert.match(provenanceVerifier, /status --porcelain=v1/);
  assert.match(provenanceVerifier, /vcs\.revision/);
  assert.match(provenanceVerifier, /vcs\.modified/);
  assert.match(provenanceVerifier, /artifacts\.fns-agent\.sha256/);
  assert.match(provenanceVerifier, /artifacts\.fns-server\.sha256/);
});

test("desktop package exposes signed acceptance and release builders", () => {
  const desktopPackage = JSON.parse(packageSource);
  assert.equal(
    desktopPackage.scripts["acceptance:macos-arm64"],
    "../../scripts/build-macos-arm64-acceptance.sh",
  );
  assert.equal(
    desktopPackage.scripts["release:macos-arm64"],
    "../../scripts/build-macos-arm64-release.sh",
  );
});

test("macOS quit acceptance requires an explicit SSH host without infrastructure defaults", () => {
  assert.match(
    quitAcceptance,
    /ssh_host_alias=\$\{FNS_ACCEPTANCE_SSH_HOST_ALIAS:-\}/,
  );
  assert.match(quitAcceptance, /\[ -n "\$ssh_host_alias" \] \|\| usage/);
  assert.doesNotMatch(quitAcceptance, /vps-|(?:\d{1,3}\.){3}\d{1,3}/);
});
