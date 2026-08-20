#!/usr/bin/env node
// 同步组件真制品校验：魔数 + 体积 + provenance sha256。打包脚本与 CI 的唯一判定。
import { createHash } from "node:crypto";
import { readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export function isInvokedDirectly(argv1, moduleUrl) {
  return Boolean(argv1) && path.resolve(argv1) === fileURLToPath(moduleUrl);
}

const MIN_BYTES = 1024; // 与运行时 is_real_artifact / is_real_sidecar 同阈值

export function isElfX8664(buf) {
  return (
    buf.length > MIN_BYTES &&
    buf[0] === 0x7f && buf[1] === 0x45 && buf[2] === 0x4c && buf[3] === 0x46 &&
    buf[4] === 2 && buf[5] === 1 &&
    buf.readUInt16LE(18) === 0x3e
  );
}

export function isMachO64(buf, cpu) {
  const type = { x64: 0x01000007, arm64: 0x0100000c }[cpu];
  return buf.length > MIN_BYTES && buf.readUInt32LE(0) === 0xfeedfacf && buf.readUInt32LE(4) === type;
}

export function isPe(buf) {
  return buf.length > MIN_BYTES && buf[0] === 0x4d && buf[1] === 0x5a;
}

function sha256(file) {
  return createHash("sha256").update(readFileSync(file)).digest("hex");
}

export function verifyRemoteDir(dir) {
  for (const name of ["fns-server", "fns-agent"]) {
    const file = path.join(dir, name);
    const buf = readFileSync(file);
    if (!isElfX8664(buf)) throw new Error(`${file} 不是 Linux x86_64 ELF（或不足 ${MIN_BYTES} 字节）`);
  }
  const provenancePath = path.join(dir, "release-provenance.json");
  const provenance = JSON.parse(readFileSync(provenancePath, "utf8"));
  const artifacts = provenance?.artifacts;
  if (!artifacts?.["fns-server"]?.sha256 || !artifacts?.["fns-agent"]?.sha256) {
    throw new Error(`${provenancePath} 缺 artifacts.*.sha256（provenance 为空壳）`);
  }
  for (const name of ["fns-server", "fns-agent"]) {
    const expected = artifacts[name].sha256;
    const actual = sha256(path.join(dir, name));
    if (expected !== actual) throw new Error(`${name} sha256 与 provenance 不符`);
    if (artifacts[name].os !== "linux" || artifacts[name].architecture !== "x86_64") {
      throw new Error(`${name} provenance os/architecture 不是 linux/x86_64`);
    }
  }
  return true;
}

export function verifySidecar(file, platform) {
  const buf = readFileSync(file);
  const ok =
    platform === "darwin-arm64" ? isMachO64(buf, "arm64")
    : platform === "darwin-x64" ? isMachO64(buf, "x64")
    : platform === "win32-x64" ? isPe(buf)
    : platform === "linux-x64" ? isElfX8664(buf)
    : false;
  if (!ok) throw new Error(`${file} 不是 ${platform} 的真 sidecar（或不足 ${MIN_BYTES} 字节）`);
  if (statSync(file).size <= MIN_BYTES) throw new Error(`${file} 体积不足 ${MIN_BYTES} 字节`);
  return true;
}

const invokedDirectly = isInvokedDirectly(process.argv[1], import.meta.url);
if (invokedDirectly) {
  const [mode, target, platform] = process.argv.slice(2);
  try {
    if (mode === "remote") verifyRemoteDir(target);
    else if (mode === "sidecar") verifySidecar(target, platform);
    else throw new Error("用法: verify.mjs remote <dir> | sidecar <file> <darwin-arm64|darwin-x64|win32-x64|linux-x64>");
    console.log(`verify ok: ${mode} ${target}`);
  } catch (error) {
    console.error(`verify failed: ${error.message}`);
    process.exit(1);
  }
}
