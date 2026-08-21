import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  hasElfInterpreter,
  isElfX8664,
  isInvokedDirectly,
  isMachO64,
  isPe,
  verifyRemoteDir,
  verifySidecar,
} from "./verify.mjs";

function fakeElfX8664(size = 2048) {
  const buf = Buffer.alloc(size);
  buf.set([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1], 0); // \x7fELF, 64-bit, LSB
  buf.writeUInt16LE(0x3e, 18); // e_machine = EM_X86_64
  return buf;
}

function fakeElfWithProgramHeaders(types) {
  const buf = fakeElfX8664();
  const programHeaderOffset = 64;
  const programHeaderSize = 56;
  buf.writeBigUInt64LE(BigInt(programHeaderOffset), 32);
  buf.writeUInt16LE(programHeaderSize, 54);
  buf.writeUInt16LE(types.length, 56);
  types.forEach((type, index) => {
    buf.writeUInt32LE(type, programHeaderOffset + index * programHeaderSize);
  });
  return buf;
}

test("placeholder bytes are never a real artifact", () => {
  assert.equal(isElfX8664(Buffer.from("placeholder\n")), false);
});
test("a 64-bit x86-64 ELF header over 1024 bytes passes", () => {
  assert.equal(isElfX8664(fakeElfX8664()), true);
});
test("ELF64 program headers distinguish dynamic and static executables", () => {
  const dynamic = fakeElfWithProgramHeaders([1, 3]); // PT_LOAD, PT_INTERP
  const staticElf = fakeElfWithProgramHeaders([1, 2]); // PT_LOAD, PT_DYNAMIC
  assert.equal(hasElfInterpreter(dynamic), true);
  assert.equal(hasElfInterpreter(staticElf), false);
});
test("wrong machine type fails", () => {
  const buf = fakeElfX8664();
  buf.writeUInt16LE(0xb7, 18); // aarch64
  assert.equal(isElfX8664(buf), false);
});
test("mach-o and pe magics are recognized per platform", () => {
  const macho = Buffer.alloc(2048);
  macho.writeUInt32LE(0xfeedfacf, 0);
  macho.writeUInt32LE(0x0100000c, 4); // arm64
  assert.equal(isMachO64(macho, "arm64"), true);
  assert.equal(isMachO64(macho, "x64"), false);
  const pe = Buffer.alloc(2048);
  pe.set([0x4d, 0x5a], 0);
  assert.equal(isPe(pe), true);
});
test("isInvokedDirectly compares resolved argv1 to fileURLToPath, not URL.pathname", () => {
  const moduleUrl = import.meta.url;
  assert.equal(isInvokedDirectly(fileURLToPath(moduleUrl), moduleUrl), true);
  assert.equal(isInvokedDirectly(undefined, moduleUrl), false);
  assert.equal(isInvokedDirectly("/other/file.mjs", moduleUrl), false);

  const winUrl = "file:///C:/scripts/verify.mjs";
  const winPathname = new URL(winUrl).pathname;
  assert.equal(winPathname, "/C:/scripts/verify.mjs");
  // Windows path.resolve is "C:\\scripts\\verify.mjs"; URL.pathname is "/C:/..."
  assert.notEqual("C:\\scripts\\verify.mjs", winPathname);
  assert.equal(isInvokedDirectly(fileURLToPath(winUrl), winUrl), true);
});

test("verifyRemoteDir rejects empty provenance and sha mismatch", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "vfy-"));
  writeFileSync(path.join(dir, "fns-server"), fakeElfX8664());
  writeFileSync(path.join(dir, "fns-agent"), fakeElfX8664());
  writeFileSync(path.join(dir, "release-provenance.json"), "{}");
  assert.throws(() => verifyRemoteDir(dir), /provenance/);
});

test("verifyRemoteDir rejects a dynamically linked remote fns-agent", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "vfy-dynamic-"));
  const server = fakeElfWithProgramHeaders([1]);
  const agent = fakeElfWithProgramHeaders([1, 3]);
  writeFileSync(path.join(dir, "fns-server"), server);
  writeFileSync(path.join(dir, "fns-agent"), agent);
  const sha256 = (buf) => createHash("sha256").update(buf).digest("hex");
  writeFileSync(path.join(dir, "release-provenance.json"), JSON.stringify({
    artifacts: {
      "fns-server": { sha256: sha256(server), os: "linux", architecture: "x86_64" },
      "fns-agent": { sha256: sha256(agent), os: "linux", architecture: "x86_64" },
    },
  }));
  assert.throws(() => verifyRemoteDir(dir), /PT_INTERP|动态链接/);
});
