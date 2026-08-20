import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { isElfX8664, isInvokedDirectly, isMachO64, isPe, verifyRemoteDir, verifySidecar } from "./verify.mjs";

function fakeElfX8664(size = 2048) {
  const buf = Buffer.alloc(size);
  buf.set([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1], 0); // \x7fELF, 64-bit, LSB
  buf.writeUInt16LE(0x3e, 18); // e_machine = EM_X86_64
  return buf;
}

test("placeholder bytes are never a real artifact", () => {
  assert.equal(isElfX8664(Buffer.from("placeholder\n")), false);
});
test("a 64-bit x86-64 ELF header over 1024 bytes passes", () => {
  assert.equal(isElfX8664(fakeElfX8664()), true);
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
