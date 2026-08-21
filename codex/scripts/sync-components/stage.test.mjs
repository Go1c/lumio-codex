import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  hostTriple,
  buildProvenance,
  cargoTargetRoot,
  isInvokedDirectly,
  resolveRemoteAgentInput,
  resolveServerSource,
} from "./stage.mjs";

test("host triple mapping covers the three shipping hosts", () => {
  assert.equal(hostTriple("darwin", "arm64"), "aarch64-apple-darwin");
  assert.equal(hostTriple("darwin", "x64"), "x86_64-apple-darwin");
  assert.equal(hostTriple("win32", "x64"), "x86_64-pc-windows-msvc");
  assert.equal(hostTriple("linux", "x64"), "x86_64-unknown-linux-gnu");
  assert.throws(() => hostTriple("sunos", "x64"));
});

test("provenance carries schema, commits and both sha256 entries", () => {
  const provenance = buildProvenance({
    clientCommit: "c".repeat(40),
    serverCommit: "s".repeat(40),
    serverSha256: "1".repeat(64),
    agentSha256: "2".repeat(64),
  });
  assert.equal(provenance.schemaVersion, "fns-release-provenance/1");
  assert.equal(provenance.artifacts["fns-server"].architecture, "x86_64");
  assert.equal(provenance.artifacts["fns-agent"].os, "linux");
});

test("isInvokedDirectly compares resolved argv1 to fileURLToPath, not URL.pathname", () => {
  const moduleUrl = import.meta.url;
  assert.equal(isInvokedDirectly(fileURLToPath(moduleUrl), moduleUrl), true);
  assert.equal(isInvokedDirectly(undefined, moduleUrl), false);
  assert.equal(isInvokedDirectly("/other/file.mjs", moduleUrl), false);

  const winUrl = "file:///C:/scripts/stage.mjs";
  const winPathname = new URL(winUrl).pathname;
  assert.equal(winPathname, "/C:/scripts/stage.mjs");
  // Windows path.resolve is "C:\\scripts\\stage.mjs"; URL.pathname is "/C:/..."
  assert.notEqual("C:\\scripts\\stage.mjs", winPathname);
  assert.equal(isInvokedDirectly(fileURLToPath(winUrl), winUrl), true);
});

test("cargo target root honors CARGO_TARGET_DIR then falls back", () => {
  assert.equal(cargoTargetRoot({ CARGO_TARGET_DIR: "/tmp/cargo-out" }, "/repo/cchaven"), "/tmp/cargo-out");
  assert.equal(cargoTargetRoot({}, "/repo/cchaven"), "/repo/cchaven/target");
});

test("server source defaults to the in-repo copy, not an external git url", () => {
  assert.equal(resolveServerSource({}, "/repo/cchaven"), "/repo/cchaven/services/fns-server");
  assert.equal(
    resolveServerSource({ FNS_SERVER_SOURCE_DIR: "/tmp/override" }, "/repo/cchaven"),
    "/tmp/override",
  );
});

test("remote agent build requires an explicit prebuilt musl artifact", () => {
  assert.equal(
    resolveRemoteAgentInput({ FNS_AGENT_LINUX_X86_64_ARTIFACT: "/tmp/musl/fns-agent" }),
    "/tmp/musl/fns-agent",
  );
  assert.throws(() => resolveRemoteAgentInput({}), /FNS_AGENT_LINUX_X86_64_ARTIFACT/);
});

test("nohup watchdog does not re-enable SIGHUP termination", () => {
  const watchdog = readFileSync(new URL("./watchdog.sh", import.meta.url), "utf8");
  assert.doesNotMatch(watchdog, /^trap .*HUP/m);
  assert.match(watchdog, /^trap .*TERM/m);
});

test("Linux smoke matches private credential modes and preserves failure logs", () => {
  const smoke = readFileSync(new URL("./smoke-linux.sh", import.meta.url), "utf8");
  assert.match(smoke, /chmod 0600 \$state\/token \$state\/agent\.json/);
  assert.match(smoke, /smoke failed: scratch=\$scratch/);
  assert.match(smoke, /tail -n 80 \$state\/agent\.stderr\.log/);
});
