import { invoke } from "@tauri-apps/api/core";

import {
  LumioCommandError,
  readRequiredCommandResult,
  type LumioCommandResult,
} from "../invoke.ts";
import type { ClaudeAuthMethod, ClaudeFileEntry, ClaudeProbeResult } from "./types.ts";

export const CLAUDE_COMMANDS = {
  probe: "lumio_claude_probe_connection",
  prepare: "lumio_claude_prepare_remote",
  sync: "lumio_claude_first_sync",
  openTerminal: "lumio_claude_open_system_terminal",
  runRemote: "lumio_claude_run_remote",
  listFiles: "lumio_claude_list_local_files",
} as const;

export interface ClaudeSshArgs {
  host: string;
  user: string;
  port: number;
  password?: string;
  keyPath?: string | null;
  auth?: ClaudeAuthMethod;
}

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

async function runClaudeCommand<T>(
  command: string,
  args: Record<string, unknown>,
): Promise<T> {
  const result = await invoke<LumioCommandResult<T>>(command, args);
  return readRequiredCommandResult(result);
}

function missingBackend(code: string): never {
  throw new LumioCommandError(code);
}

export function probeErrorCopy(code: string | null, host: string, port: number): string {
  switch (code) {
    case "SSH_AUTH_FAILED":
      return `无法登录 ${host}。`;
    case "SSH_UNREACHABLE":
      return `连不上 ${host}:${port}。`;
    case "SSH_NOT_SSH":
      return `${host}:${port} 不是 SSH 服务。`;
    case "SSH_CLIENT_MISSING":
      return "这台电脑还没有 ssh 命令。";
    case "SSH_HOST_REQUIRED":
      return "先填写公网 IP。";
    default:
      return `连不上这台服务器。`;
  }
}

export async function probeClaudeConnection(input: ClaudeSshArgs): Promise<ClaudeProbeResult> {
  const host = input.host.trim();
  const user = input.user.trim() || "root";
  const port = input.port || 22;
  const target = `${host}:${port}`;
  if (host === "") {
    return {
      ok: false,
      reachable: false,
      authenticated: false,
      target,
      user,
      distro: null,
      cpu: null,
      memory: null,
      errorCode: "SSH_HOST_REQUIRED",
      detail: probeErrorCopy("SSH_HOST_REQUIRED", host, port),
    };
  }

  if (!isTauri()) {
    return {
      ok: false,
      reachable: false,
      authenticated: false,
      target,
      user,
      distro: null,
      cpu: null,
      memory: null,
      errorCode: "SSH_CLIENT_MISSING",
      detail: probeErrorCopy("SSH_CLIENT_MISSING", host, port),
    };
  }

  try {
    const payload = await runClaudeCommand<{
      ok: boolean;
      reachable: boolean;
      authenticated: boolean;
      distro: string | null;
      cpu: string | null;
      memory: string | null;
      errorCode: string | null;
      detail: string | null;
    }>(CLAUDE_COMMANDS.probe, {
      host,
      user,
      port,
      password: input.password || null,
      keyPath: input.keyPath || null,
      auth: input.auth ?? (input.keyPath ? "key" : "password"),
    });
    const errorCode = payload.ok ? null : (payload.errorCode ?? "SSH_PROBE_FAILED");
    return {
      ok: payload.ok,
      reachable: payload.reachable,
      authenticated: payload.authenticated,
      target,
      user,
      distro: payload.distro,
      cpu: payload.cpu,
      memory: payload.memory,
      errorCode,
      detail: payload.detail ?? (errorCode ? probeErrorCopy(errorCode, host, port) : null),
    };
  } catch (error: unknown) {
    const errorCode = error instanceof LumioCommandError ? error.errorCode : "SSH_PROBE_FAILED";
    return {
      ok: false,
      reachable: false,
      authenticated: false,
      target,
      user,
      distro: null,
      cpu: null,
      memory: null,
      errorCode,
      detail: probeErrorCopy(errorCode, host, port),
    };
  }
}

export async function prepareClaudeRemote(input: ClaudeSshArgs & {
  remoteRoot: string;
  localRoot: string;
}): Promise<{ ok: boolean; errorCode: string | null; detail: string | null }> {
  if (!isTauri()) {
    return { ok: true, errorCode: null, detail: "本机目录将在首次同步时创建。" };
  }
  try {
    return await runClaudeCommand(CLAUDE_COMMANDS.prepare, {
      host: input.host,
      user: input.user,
      port: input.port,
      password: input.password || null,
      keyPath: input.keyPath || null,
      auth: input.auth ?? "password",
      remoteRoot: input.remoteRoot,
      localRoot: input.localRoot,
    });
  } catch (error: unknown) {
    const errorCode = error instanceof LumioCommandError ? error.errorCode : "SSH_PREPARE_FAILED";
    return { ok: false, errorCode, detail: "没能在服务器上建好项目目录。" };
  }
}

export async function firstClaudeSync(input: ClaudeSshArgs & {
  remoteRoot: string;
  localRoot: string;
}): Promise<{ ok: boolean; filesDone: number; filesTotal: number; errorCode: string | null }> {
  if (!isTauri()) {
    return { ok: true, filesDone: 0, filesTotal: 0, errorCode: "SYNC_ENGINE_UNAVAILABLE" };
  }
  try {
    return await runClaudeCommand(CLAUDE_COMMANDS.sync, {
      host: input.host,
      user: input.user,
      port: input.port,
      password: input.password || null,
      keyPath: input.keyPath || null,
      auth: input.auth ?? "password",
      remoteRoot: input.remoteRoot,
      localRoot: input.localRoot,
    });
  } catch (error: unknown) {
    const errorCode = error instanceof LumioCommandError ? error.errorCode : "SYNC_FAILED";
    return { ok: false, filesDone: 0, filesTotal: 0, errorCode };
  }
}

export async function openClaudeSystemTerminal(input: {
  host: string;
  user: string;
  port: number;
}): Promise<void> {
  if (!isTauri()) {
    missingBackend("SSH_CLIENT_MISSING");
  }
  await runClaudeCommand(CLAUDE_COMMANDS.openTerminal, {
    host: input.host,
    user: input.user,
    port: input.port,
  });
}

export async function runClaudeRemote(
  input: ClaudeSshArgs & { command: string },
): Promise<{ stdout: string; stderr: string; code: number }> {
  if (!isTauri()) {
    return { stdout: "", stderr: "需要启动器才能在服务器上执行命令。", code: 1 };
  }
  return runClaudeCommand(CLAUDE_COMMANDS.runRemote, {
    host: input.host,
    user: input.user,
    port: input.port,
    password: input.password || null,
    keyPath: input.keyPath || null,
    auth: input.auth ?? "password",
    command: input.command,
  });
}

export async function listClaudeLocalFiles(localRoot: string): Promise<ClaudeFileEntry[]> {
  if (!isTauri()) return [];
  try {
    return await runClaudeCommand<ClaudeFileEntry[]>(CLAUDE_COMMANDS.listFiles, { localRoot });
  } catch {
    return [];
  }
}
