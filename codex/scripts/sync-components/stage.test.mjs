import assert from "node:assert/strict";
import test from "node:test";
import { hostTriple, buildProvenance, cargoTargetRoot } from "./stage.mjs";

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

test("cargo target root honors CARGO_TARGET_DIR then falls back", () => {
  assert.equal(cargoTargetRoot({ CARGO_TARGET_DIR: "/tmp/cargo-out" }, "/repo/cchaven"), "/tmp/cargo-out");
  assert.equal(cargoTargetRoot({}, "/repo/cchaven"), "/repo/cchaven/target");
});
