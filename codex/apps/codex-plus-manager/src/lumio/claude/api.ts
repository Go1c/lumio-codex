import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import {
  LumioCommandError,
  readRequiredCommandResult,
  type LumioCommandResult,
} from "../invoke.ts";
import type {
  ClaudeAuthMethod,
  ClaudeConflictDiff,
  ClaudeConflictEntry,
  ClaudeConflictResolution,
  ClaudeFileEntry,
  ClaudeFilePreview,
  ClaudeProbeResult,
  ClaudeSshHost,
} from "./types.ts";

export const CLAUDE_COMMANDS = {
  probe: "lumio_claude_probe_connection",
  inspect: "lumio_claude_inspect_remote",
  prepare: "lumio_claude_prepare_remote",
  sync: "lumio_claude_first_sync",
  openTerminal: "lumio_claude_open_system_terminal",
  runRemote: "lumio_claude_run_remote",
  listFiles: "lumio_claude_list_local_files",
  listTree: "lumio_claude_list_files",
  previewFile: "lumio_claude_preview_file",
  listConflicts: "lumio_claude_list_conflicts",
  resolveConflict: "lumio_claude_resolve_conflict",
  conflictDiff: "lumio_claude_conflict_diff",
  listSshHosts: "lumio_claude_list_ssh_hosts",
  startTerminal: "lumio_claude_start_terminal",
  writeTerminal: "lumio_claude_write_terminal",
  resizeTerminal: "lumio_claude_resize_terminal",
} as const;

export const CLAUDE_SYNC_PROGRESS_EVENT = "lumio://claude-sync-progress";

export interface ClaudeSshArgs {
  host: string;
  user: string;
  port: number;
  password?: string;
  keyPath?: string | null;
  hostAlias?: string | null;
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

function sshPayload(input: ClaudeSshArgs): Record<string, unknown> {
  return {
    host: input.host,
    user: input.user,
    port: input.port,
    password: input.password || null,
    keyPath: input.keyPath || null,
    hostAlias: input.hostAlias || null,
    auth: input.auth ?? (input.keyPath ? "key" : input.hostAlias ? "config" : "password"),
  };
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
    case "SSH_PREPARE_FAILED":
      return "没能在服务器上装好同步组件。";
    case "DEPLOY_ARTIFACT_MISSING":
      return "这个版本的 BestCodex 没有把同步组件打进来，不是服务器的问题。更新或重装 BestCodex 后再试。";
    case "SSH_ALIAS_UNKNOWN":
      return "本机 SSH 配置里没有这个 Host 别名。";
    default:
      return `连不上这台服务器。`;
  }
}

export function prepareErrorCopy(code: string | null, host: string, port: number): string {
  switch (code) {
    case "SSH_PREPARE_FAILED":
      return "没能在服务器上装好同步组件。";
    case "DEPLOY_ARTIFACT_MISSING":
      return "这个版本的 BestCodex 没有把同步组件打进来，不是服务器的问题。更新或重装 BestCodex 后再试。";
    case "SSH_AUTH_FAILED":
    case "SSH_UNREACHABLE":
    case "SSH_NOT_SSH":
    case "SSH_CLIENT_MISSING":
    case "SSH_ALIAS_UNKNOWN":
      return probeErrorCopy(code, host, port);
    default:
      return "没能在服务器上装好同步组件。";
  }
}

export function syncErrorCopy(code: string | null): string {
  switch (code) {
    case "SYNC_ENGINE_UNAVAILABLE":
      return "这个版本的 BestCodex 没有把同步组件打进来，暂时拉不了文件。更新或重装 BestCodex 后再试。";
    case "SYNC_COPY_UNCONFIRMED":
      return "还没把服务器上的文件拉到这台电脑。";
    case "SSH_ALIAS_UNKNOWN":
      return "本机 SSH 配置里没有这个 Host 别名。";
    default:
      return "没能把服务器上的文件拉到这台电脑。";
  }
}

export async function probeClaudeConnection(input: ClaudeSshArgs): Promise<ClaudeProbeResult> {
  const host = input.host.trim();
  const user = input.user.trim() || "root";
  const port = input.port || 22;
  const target = `${host}:${port}`;
  if (host === "" && !input.hostAlias) {
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
    }>(CLAUDE_COMMANDS.probe, sshPayload({ ...input, host, user, port }));
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

export async function inspectClaudeRemote(input: ClaudeSshArgs & {
  remoteRoot: string;
}): Promise<{
  ok: boolean;
  exists: boolean;
  names: string[];
  errorCode: string | null;
  detail: string | null;
}> {
  if (!isTauri()) {
    return {
      ok: false,
      exists: false,
      names: [],
      errorCode: "SSH_CLIENT_MISSING",
      detail: prepareErrorCopy("SSH_CLIENT_MISSING", input.host, input.port),
    };
  }
  try {
    return await runClaudeCommand(CLAUDE_COMMANDS.inspect, {
      ...sshPayload(input),
      remoteRoot: input.remoteRoot,
    });
  } catch (error: unknown) {
    const errorCode = error instanceof LumioCommandError ? error.errorCode : "SSH_PREPARE_FAILED";
    return {
      ok: false,
      exists: false,
      names: [],
      errorCode,
      detail: prepareErrorCopy(errorCode, input.host, input.port),
    };
  }
}

export async function prepareClaudeRemote(input: ClaudeSshArgs & {
  remoteRoot: string;
  localRoot: string;
}): Promise<{ ok: boolean; errorCode: string | null; detail: string | null }> {
  if (!isTauri()) {
    return { ok: false, errorCode: "SSH_CLIENT_MISSING", detail: prepareErrorCopy("SSH_CLIENT_MISSING", input.host, input.port) };
  }
  try {
    return await runClaudeCommand(CLAUDE_COMMANDS.prepare, {
      ...sshPayload(input),
      remoteRoot: input.remoteRoot,
      localRoot: input.localRoot,
    });
  } catch (error: unknown) {
    const errorCode = error instanceof LumioCommandError ? error.errorCode : "SSH_PREPARE_FAILED";
    return { ok: false, errorCode, detail: prepareErrorCopy(errorCode, input.host, input.port) };
  }
}

export async function firstClaudeSync(input: ClaudeSshArgs & {
  remoteRoot: string;
  localRoot: string;
  projectId?: string;
}): Promise<{ ok: boolean; filesDone: number; filesTotal: number; errorCode: string | null }> {
  if (!isTauri()) {
    return { ok: false, filesDone: 0, filesTotal: 0, errorCode: "SYNC_ENGINE_UNAVAILABLE" };
  }
  try {
    return await runClaudeCommand(CLAUDE_COMMANDS.sync, {
      ...sshPayload(input),
      remoteRoot: input.remoteRoot,
      localRoot: input.localRoot,
      projectId: input.projectId ?? null,
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
    ...sshPayload(input),
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

export async function listClaudeFiles(input: ClaudeSshArgs & {
  localRoot: string;
  remoteRoot: string;
}): Promise<{ local: ClaudeFileEntry[]; remote: ClaudeFileEntry[] }> {
  if (!isTauri()) return { local: [], remote: [] };
  try {
    return await runClaudeCommand(CLAUDE_COMMANDS.listTree, {
      ...sshPayload(input),
      localRoot: input.localRoot,
      remoteRoot: input.remoteRoot,
    });
  } catch {
    return { local: [], remote: [] };
  }
}

export async function previewClaudeFile(input: ClaudeSshArgs & {
  localRoot: string;
  remoteRoot: string;
  path: string;
  side: "local" | "remote";
}): Promise<ClaudeFilePreview | null> {
  if (!isTauri()) return null;
  try {
    return await runClaudeCommand(CLAUDE_COMMANDS.previewFile, {
      ...sshPayload(input),
      localRoot: input.localRoot,
      remoteRoot: input.remoteRoot,
      path: input.path,
      side: input.side,
    });
  } catch {
    return null;
  }
}

export async function listClaudeConflicts(input: {
  projectId: string;
  localRoot: string;
}): Promise<ClaudeConflictEntry[]> {
  if (!isTauri()) return [];
  try {
    return await runClaudeCommand(CLAUDE_COMMANDS.listConflicts, {
      projectId: input.projectId,
      localRoot: input.localRoot,
    });
  } catch {
    return [];
  }
}

export async function resolveClaudeConflict(input: {
  projectId: string;
  localRoot: string;
  conflictId: string;
  resolution: ClaudeConflictResolution;
}): Promise<{ remaining: number; copyPath: string | null }> {
  if (!isTauri()) {
    missingBackend("SSH_CLIENT_MISSING");
  }
  return runClaudeCommand(CLAUDE_COMMANDS.resolveConflict, input);
}

export async function diffClaudeConflict(input: {
  projectId: string;
  localRoot: string;
  conflictId: string;
}): Promise<ClaudeConflictDiff | null> {
  if (!isTauri()) return null;
  try {
    return await runClaudeCommand(CLAUDE_COMMANDS.conflictDiff, input);
  } catch {
    return null;
  }
}

export async function listClaudeSshHosts(): Promise<ClaudeSshHost[]> {
  if (!isTauri()) return [];
  try {
    return await runClaudeCommand(CLAUDE_COMMANDS.listSshHosts, {});
  } catch {
    return [];
  }
}

export async function startClaudeTerminal(input: ClaudeSshArgs & {
  projectId: string;
  remoteRoot: string;
  cols: number;
  rows: number;
}): Promise<void> {
  if (!isTauri()) {
    missingBackend("SSH_CLIENT_MISSING");
  }
  await runClaudeCommand(CLAUDE_COMMANDS.startTerminal, {
    ...sshPayload(input),
    projectId: input.projectId,
    remoteRoot: input.remoteRoot,
    cols: input.cols,
    rows: input.rows,
  });
}

export async function writeClaudeTerminal(projectId: string, bytes: number[]): Promise<void> {
  if (!isTauri()) return;
  await runClaudeCommand(CLAUDE_COMMANDS.writeTerminal, { projectId, bytes });
}

export async function resizeClaudeTerminal(projectId: string, cols: number, rows: number): Promise<void> {
  if (!isTauri()) return;
  await runClaudeCommand(CLAUDE_COMMANDS.resizeTerminal, { projectId, cols, rows });
}

export function terminalOutputEvent(projectId: string): string {
  return `lumio://claude-terminal-output-${projectId}`;
}

export function terminalClosedEvent(projectId: string): string {
  return `lumio://claude-terminal-closed-${projectId}`;
}

export async function subscribeClaudeEvent<T>(
  event: string,
  handler: (payload: T) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return () => undefined;
  return listen<T>(event, (incoming) => handler(incoming.payload));
}
