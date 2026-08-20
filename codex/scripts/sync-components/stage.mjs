#!/usr/bin/env node
// 同步组件唯一暂存入口。
// 用法: node stage.mjs [--dev | --build-remote]
// 环境: BESTCODEX_FNS_PREBUILT_DIR   远端对预制目录（CI artifact 下载处 / 本机缓存）
//       FNS_SERVER_SOURCE_DIR        本机 fns-server 源 checkout（--build-remote 用）
//       FNS_SERVER_GIT_URL           无本机源时的 git 地址（配 fns-server.pin.json 的 commit）
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { verifyRemoteDir, verifySidecar } from "./verify.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const codexRoot = path.resolve(here, "..", "..");
const repoRoot = path.resolve(codexRoot, "..");
const cchavenRoot = path.join(repoRoot, "cchaven");
const srcTauri = path.join(codexRoot, "apps", "codex-plus-manager", "src-tauri");
const remoteStage = path.join(srcTauri, "resources", "remote", "linux-x86_64");
const prebuiltDir =
  process.env.BESTCODEX_FNS_PREBUILT_DIR ??
  path.join(codexRoot, "target", "sync-components", "remote-linux-x86_64");

export function isInvokedDirectly(argv1, moduleUrl) {
  return Boolean(argv1) && path.resolve(argv1) === fileURLToPath(moduleUrl);
}

export function cargoTargetRoot(env = process.env, root = cchavenRoot) {
  return env.CARGO_TARGET_DIR ?? path.join(root, "target");
}

export function hostTriple(platform = process.platform, arch = process.arch) {
  const map = {
    "darwin-arm64": "aarch64-apple-darwin",
    "darwin-x64": "x86_64-apple-darwin",
    "win32-x64": "x86_64-pc-windows-msvc",
    "linux-x64": "x86_64-unknown-linux-gnu",
  };
  const triple = map[`${platform}-${arch}`];
  if (!triple) throw new Error(`不支持的宿主: ${platform}-${arch}`);
  return triple;
}

export function buildProvenance({ clientCommit, serverCommit, serverSha256, agentSha256 }) {
  const now = new Date().toISOString().replace(/\.\d+Z$/, "Z");
  const meta = (sha256) => ({
    sha256, os: "linux", architecture: "x86_64", buildTimestamp: now,
  });
  return {
    schemaVersion: "fns-release-provenance/1",
    buildTimestamp: now,
    builder: `${process.platform}/node-${process.versions.node}`,
    clientCommit, serverCommit,
    artifacts: { "fns-server": meta(serverSha256), "fns-agent": meta(agentSha256) },
  };
}

function run(cmd, args, options = {}) {
  const result = spawnSync(cmd, args, { stdio: "inherit", ...options });
  if (result.status !== 0) throw new Error(`${cmd} ${args.join(" ")} 失败（exit ${result.status}）`);
}

function sha256(file) {
  return createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

function stageHostSidecar() {
  const triple = hostTriple();
  run("cargo", ["build", "--locked", "--release", "--target", triple, "-p", "fns-agent", "--bin", "fns-agent"], { cwd: cchavenRoot });
  const ext = process.platform === "win32" ? ".exe" : "";
  const built = path.join(cargoTargetRoot(), triple, "release", `fns-agent${ext}`);
  const dest = path.join(srcTauri, "binaries", `fns-agent-${triple}${ext}`);
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.copyFileSync(built, dest);
  fs.chmodSync(dest, 0o755);
  verifySidecar(dest, `${process.platform === "win32" ? "win32" : process.platform}-${process.arch}`);
  console.log(`staged host sidecar: ${dest}`);
}

function buildRemote() {
  fs.mkdirSync(prebuiltDir, { recursive: true });
  const agentOut = path.join(prebuiltDir, "fns-agent");
  if (process.platform === "linux") {
    run("cargo", ["build", "--locked", "--release", "--target", "x86_64-unknown-linux-gnu", "-p", "fns-agent", "--bin", "fns-agent"], { cwd: cchavenRoot });
    fs.copyFileSync(path.join(cargoTargetRoot(), "x86_64-unknown-linux-gnu", "release", "fns-agent"), agentOut);
    fs.chmodSync(agentOut, 0o755);
  } else if (!fs.existsSync(agentOut)) {
    throw new Error(`非 Linux 宿主无法编 Linux fns-agent；请先把产物放进 ${prebuiltDir}（CI 由 ubuntu job 提供）`);
  }
  const pin = JSON.parse(fs.readFileSync(path.join(here, "fns-server.pin.json"), "utf8"));
  let serverSource = process.env.FNS_SERVER_SOURCE_DIR;
  if (!serverSource) {
    const gitUrl = process.env.FNS_SERVER_GIT_URL || pin.url;
    if (!gitUrl) throw new Error("缺 FNS_SERVER_SOURCE_DIR 或 FNS_SERVER_GIT_URL：fns-server 无源可编");
    serverSource = fs.mkdtempSync(path.join(codexRoot, "target", "fns-server-src-"));
    run("git", ["clone", "--no-checkout", gitUrl, serverSource]);
    run("git", ["checkout", "--detach", pin.commit], { cwd: serverSource });
  }
  const serverOut = path.join(prebuiltDir, "fns-server");
  run("go", ["build", "-trimpath", "-o", serverOut, "."], {
    cwd: serverSource,
    env: { ...process.env, CGO_ENABLED: "0", GOOS: "linux", GOARCH: "amd64" },
  });
  fs.chmodSync(serverOut, 0o755);
  const clientCommit = spawnSync("git", ["rev-parse", "HEAD"], { cwd: repoRoot, encoding: "utf8" }).stdout.trim();
  const serverCommit = spawnSync("git", ["rev-parse", "HEAD"], { cwd: serverSource, encoding: "utf8" }).stdout.trim();
  const provenance = buildProvenance({
    clientCommit, serverCommit,
    serverSha256: sha256(serverOut), agentSha256: sha256(agentOut),
  });
  fs.writeFileSync(path.join(prebuiltDir, "release-provenance.json"), JSON.stringify(provenance, null, 2));
  verifyRemoteDir(prebuiltDir);
  console.log(`built remote components: ${prebuiltDir}`);
}

function stageRemote({ dev }) {
  try {
    verifyRemoteDir(prebuiltDir);
  } catch (error) {
    const message = `远端同步组件不可用（${error.message}）。这个开发包不会打进同步组件：` +
      `「装组件」会明确失败。要补齐请先跑 node codex/scripts/sync-components/stage.mjs --build-remote`;
    if (dev) { console.warn(`\n[stage:sync-components] 警告：${message}\n`); return; }
    throw new Error(message);
  }
  fs.mkdirSync(remoteStage, { recursive: true });
  for (const name of ["fns-server", "fns-agent", "release-provenance.json"]) {
    fs.copyFileSync(path.join(prebuiltDir, name), path.join(remoteStage, name));
  }
  fs.chmodSync(path.join(remoteStage, "fns-server"), 0o755);
  fs.chmodSync(path.join(remoteStage, "fns-agent"), 0o755);
  verifyRemoteDir(remoteStage);
  console.log(`staged remote components: ${remoteStage}`);
}

const invokedDirectly = isInvokedDirectly(process.argv[1], import.meta.url);
if (invokedDirectly) {
  const mode = process.argv.includes("--build-remote") ? "build-remote"
    : process.argv.includes("--dev") ? "dev" : "strict";
  try {
    if (mode === "build-remote") buildRemote();
    else { stageHostSidecar(); stageRemote({ dev: mode === "dev" }); }
  } catch (error) {
    console.error(`[stage:sync-components] ${error.message}`);
    process.exit(1);
  }
}
