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
  ClaudeResumeResult,
  ClaudeServerStatus,
  ClaudeSessionsSnapshot,
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
  localFs: "lumio_claude_local_fs",
  listConflicts: "lumio_claude_list_conflicts",
  resolveConflict: "lumio_claude_resolve_conflict",
  conflictDiff: "lumio_claude_conflict_diff",
  listSshHosts: "lumio_claude_list_ssh_hosts",
  startTerminal: "lumio_claude_start_terminal",
  writeTerminal: "lumio_claude_write_terminal",
  resizeTerminal: "lumio_claude_resize_terminal",
  resume: "lumio_claude_resume_sync",
  serverStatus: "lumio_claude_server_status",
  listSessions: "lumio_claude_list_sessions",
  installCli: "lumio_claude_install_cli",
  loginStart: "lumio_claude_login_start",
  loginSubmit: "lumio_claude_login_submit",
  loginStatus: "lumio_claude_login_status",
  openChat: "lumio_claude_open_chat",
  closeChat: "lumio_claude_close_chat",
  listChats: "lumio_claude_list_chats",
} as const;

export const CLAUDE_SYNC_PROGRESS_EVENT = "lumio://claude-sync-progress";
export const CLAUDE_PREPARE_PROGRESS_EVENT = "lumio://claude-prepare-progress";
export const CLAUDE_CLI_PROGRESS_EVENT = "lumio://claude-cli-progress";
export const CLAUDE_LOGIN_PROGRESS_EVENT = "lumio://claude-login-progress";
export const DEFAULT_TERMINAL_SESSION_ID = "default";

export function setupPhaseCopy(phase: string, uploadIndex = 1): string {
  switch (phase) {
    case "inspect":
      return "正在检查服务器…";
    case "mkdir":
      return "正在服务器上创建项目目录…";
    case "upload":
      return uploadIndex === 2
        ? "正在把同步组件传到服务器（2 / 2）…"
        : "正在把同步组件传到服务器（1 / 2）…";
    case "finish":
      return "正在完成安装…";
    default:
      return "正在准备…";
  }
}

export function formatSetupElapsed(seconds: number): string {
  return `已用 ${seconds} 秒`;
}

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

export function localFsErrorCopy(code: string | null): string {
  switch (code) {
    case "FILE_EXISTS":
      return "已经有这个名字。";
    case "FILE_MISSING":
      return "找不到这个文件。";
    case "FILE_NAME_INVALID":
      return "这个名字不能用。";
    case "FILE_REVEAL_FAILED":
      return "没能在 Finder 里打开。";
    case "PATH_OUTSIDE_PROJECT":
      return "路径必须位于项目文件夹内。";
    case "FILE_WRITE_FAILED":
    default:
      if (code && /[\u4e00-\u9fff]/.test(code)) return code;
      return "没能改这个文件。";
  }
}

export function syncErrorCopy(code: string | null): string {
  switch (code) {
    case "SYNC_ENGINE_UNAVAILABLE":
      return "这个版本的 BestCodex 没有把同步组件打进来，暂时拉不了文件。更新或重装 BestCodex 后再试。";
    case "SYNC_REMOTE_NOT_RUNNING":
      return "服务器上的同步组件没有在运行，文件还没同步过去。";
    case "SYNC_COPY_UNCONFIRMED":
      return "还没把服务器上的文件拉到这台电脑。";
    case "SSH_ALIAS_UNKNOWN":
      return "本机 SSH 配置里没有这个 Host 别名。";
    default:
      return "没能把服务器上的文件拉到这台电脑。";
  }
}

export function cliErrorCopy(code: string | null): string {
  switch (code) {
    case "CLAUDE_CLI_NO_NETWORK":
      return "这台服务器现在连不上外网。检查网络后再试。";
    case "CLAUDE_CLI_DNS":
      return "这台服务器解析不了官方下载地址。检查 DNS 后再试。";
    case "CLAUDE_CLI_NO_CURL":
      return "这台服务器没有 curl，装不上官方 Claude。";
    case "CLAUDE_CLI_BIN_UNWRITABLE":
      return "写不进 ~/.local/bin，没法安装 Claude。";
    case "CLAUDE_CLI_DOWNLOAD_FAILED":
      return "服务器连不上官方下载地址。确认这台服务器能访问外网，或稍后再试。";
    case "CLAUDE_CLI_VERIFY_FAILED":
      return "装完之后没能读到 Claude 版本，安装可能没有成功。";
    case "CLAUDE_CLI_INSTALL_FAILED":
      return "没能在这台服务器上装好 Claude。";
    case "SSH_CLIENT_MISSING":
      return "这台电脑还没有 ssh 命令。";
    case "SSH_ALIAS_UNKNOWN":
      return "本机 SSH 配置里没有这个 Host 别名。";
    default:
      return "没能在这台服务器上装好 Claude。";
  }
}

export function loginErrorCopy(code: string | null): string {
  switch (code) {
    case "CLAUDE_LOGIN_NO_CLI":
      return "服务器上还没有官方 Claude 命令。";
    case "CLAUDE_LOGIN_NO_URL":
      return "没能拿到登录链接。";
    case "CLAUDE_LOGIN_CODE_REJECTED":
      return "授权码未被接受。";
    case "CLAUDE_LOGIN_EXPIRED":
      return "登录已过期。";
    case "CLAUDE_LOGIN_FAILED":
      return "没能完成 Anthropic 登录。";
    case "SSH_AUTH_FAILED":
      return "无法登录这台服务器。";
    case "SSH_UNREACHABLE":
      return "连不上这台服务器。";
    default:
      return "没能完成 Anthropic 登录。";
  }
}

export interface ClaudeCliEnsureResult {
  ok: boolean;
  phase: string;
  version: string | null;
  latest: string | null;
  errorCode: string | null;
  detail: string | null;
}

export interface ClaudeLoginStartResult {
  ok: boolean;
  loginUrl: string | null;
  errorCode: string | null;
  detail: string | null;
}

export interface ClaudeLoginSubmitResult {
  ok: boolean;
  phase: string;
  errorCode: string | null;
  detail: string | null;
}

export interface ClaudeLoginStatusResult {
  phase: string;
  errorCode: string | null;
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

export async function resumeOfficialSync(input: ClaudeSshArgs & {
  remoteRoot: string;
  localRoot: string;
  projectId: string;
}): Promise<ClaudeResumeResult> {
  if (!isTauri()) {
    return {
      ok: false,
      running: false,
      filesDone: 0,
      filesTotal: 0,
      errorCode: "SYNC_ENGINE_UNAVAILABLE",
    };
  }
  try {
    return await runClaudeCommand(CLAUDE_COMMANDS.resume, {
      ...sshPayload(input),
      remoteRoot: input.remoteRoot,
      localRoot: input.localRoot,
      projectId: input.projectId,
    });
  } catch (error: unknown) {
    const errorCode = error instanceof LumioCommandError ? error.errorCode : "SYNC_FAILED";
    return { ok: false, running: false, filesDone: 0, filesTotal: 0, errorCode };
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

export async function mutateClaudeLocalFile(input: {
  localRoot: string;
  action:
    | "create-file"
    | "create-folder"
    | "duplicate"
    | "rename"
    | "delete"
    | "reveal"
    | "open-folder"
    | "open-file";
  path: string;
  isDir?: boolean;
  name?: string;
}): Promise<string> {
  if (!isTauri()) {
    missingBackend("FILE_WRITE_FAILED");
  }
  const payload = await runClaudeCommand<{ path: string }>(CLAUDE_COMMANDS.localFs, {
    localRoot: input.localRoot,
    action: input.action,
    path: input.path,
    isDir: input.isDir ?? false,
    name: input.name ?? null,
  });
  return payload.path;
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

export async function loadClaudeServerStatus(input: ClaudeSshArgs & {
  projectId: string;
  remoteRoot: string;
}): Promise<ClaudeServerStatus> {
  if (!isTauri()) {
    return {
      projectId: input.projectId,
      capturedAt: String(Date.now()),
      ok: false,
      error: { code: "SSH_CLIENT_MISSING", message: "需要启动器才能读取服务器状态。" },
    };
  }
  try {
    return await runClaudeCommand(CLAUDE_COMMANDS.serverStatus, {
      ...sshPayload(input),
      projectId: input.projectId,
      remoteRoot: input.remoteRoot,
    });
  } catch (error: unknown) {
    const code = error instanceof LumioCommandError ? error.errorCode : "SSH_PROBE_FAILED";
    return {
      projectId: input.projectId,
      capturedAt: String(Date.now()),
      ok: false,
      error: { code, message: "没能读取服务器状态。" },
    };
  }
}

export async function loadClaudeSessions(input: ClaudeSshArgs & {
  projectId: string;
}): Promise<ClaudeSessionsSnapshot> {
  if (!isTauri()) {
    return {
      projectId: input.projectId,
      capturedAt: String(Date.now()),
      ok: false,
      sessionExists: false,
      windows: [],
      error: { code: "SSH_CLIENT_MISSING", message: "需要启动器才能读取对话状态。" },
    };
  }
  try {
    return await runClaudeCommand(CLAUDE_COMMANDS.listSessions, {
      ...sshPayload(input),
      projectId: input.projectId,
    });
  } catch (error: unknown) {
    const code = error instanceof LumioCommandError ? error.errorCode : "SSH_PROBE_FAILED";
    return {
      projectId: input.projectId,
      capturedAt: String(Date.now()),
      ok: false,
      sessionExists: false,
      windows: [],
      error: { code, message: "没能读取对话状态。" },
    };
  }
}

export async function startClaudeTerminal(input: ClaudeSshArgs & {
  projectId: string;
  remoteRoot: string;
  cols: number;
  rows: number;
  sessionId?: string;
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
    sessionId: input.sessionId ?? DEFAULT_TERMINAL_SESSION_ID,
  });
}

export async function writeClaudeTerminal(
  projectId: string,
  bytes: number[],
  sessionId = DEFAULT_TERMINAL_SESSION_ID,
): Promise<void> {
  if (!isTauri()) return;
  await runClaudeCommand(CLAUDE_COMMANDS.writeTerminal, { projectId, bytes, sessionId });
}

export async function resizeClaudeTerminal(
  projectId: string,
  cols: number,
  rows: number,
  sessionId = DEFAULT_TERMINAL_SESSION_ID,
): Promise<void> {
  if (!isTauri()) return;
  await runClaudeCommand(CLAUDE_COMMANDS.resizeTerminal, { projectId, cols, rows, sessionId });
}

export async function openClaudeChat(input: ClaudeSshArgs & {
  projectId: string;
  sessionId: string;
  remoteRoot: string;
  cols: number;
  rows: number;
}): Promise<void> {
  if (!isTauri()) {
    missingBackend("SSH_CLIENT_MISSING");
  }
  await runClaudeCommand(CLAUDE_COMMANDS.openChat, {
    ...sshPayload(input),
    projectId: input.projectId,
    sessionId: input.sessionId,
    remoteRoot: input.remoteRoot,
    cols: input.cols,
    rows: input.rows,
  });
}

export async function closeClaudeChat(input: ClaudeSshArgs & {
  projectId: string;
  sessionId: string;
}): Promise<void> {
  if (!isTauri()) return;
  try {
    await runClaudeCommand(CLAUDE_COMMANDS.closeChat, {
      ...sshPayload(input),
      projectId: input.projectId,
      sessionId: input.sessionId,
    });
  } catch {
    /* closing a missing session is not a user-facing failure */
  }
}

export async function listClaudeChats(projectId: string): Promise<string[]> {
  if (!isTauri()) return [];
  try {
    return await runClaudeCommand<string[]>(CLAUDE_COMMANDS.listChats, { projectId });
  } catch {
    return [];
  }
}

export async function installClaudeCli(input: ClaudeSshArgs): Promise<ClaudeCliEnsureResult> {
  if (!isTauri()) {
    return {
      ok: false,
      phase: "fail",
      version: null,
      latest: null,
      errorCode: "SSH_CLIENT_MISSING",
      detail: cliErrorCopy("SSH_CLIENT_MISSING"),
    };
  }
  try {
    const payload = await runClaudeCommand<ClaudeCliEnsureResult>(CLAUDE_COMMANDS.installCli, {
      ...sshPayload(input),
      channel: "latest",
    });
    const errorCode = payload.ok ? payload.errorCode : (payload.errorCode ?? "CLAUDE_CLI_INSTALL_FAILED");
    return {
      ok: payload.ok,
      phase: payload.phase,
      version: payload.version ?? null,
      latest: payload.latest ?? null,
      errorCode,
      detail: payload.detail ?? (errorCode ? cliErrorCopy(errorCode) : null),
    };
  } catch (error: unknown) {
    const errorCode = error instanceof LumioCommandError ? error.errorCode : "CLAUDE_CLI_INSTALL_FAILED";
    return {
      ok: false,
      phase: "fail",
      version: null,
      latest: null,
      errorCode,
      detail: cliErrorCopy(errorCode),
    };
  }
}

function unwrapLoginResult<T>(raw: T | LumioCommandResult<T>): T {
  if (raw && typeof raw === "object" && "payload" in raw && "ok" in raw) {
    return readRequiredCommandResult(raw as LumioCommandResult<T>);
  }
  return raw;
}

async function runLoginCommand<T>(command: string, args: Record<string, unknown>): Promise<T> {
  if (!isTauri()) {
    missingBackend("SSH_CLIENT_MISSING");
  }
  const raw = await invoke<T | LumioCommandResult<T>>(command, args);
  return unwrapLoginResult(raw);
}

export async function startClaudeLogin(input: ClaudeSshArgs): Promise<ClaudeLoginStartResult> {
  if (!isTauri()) {
    return {
      ok: false,
      loginUrl: null,
      errorCode: "SSH_CLIENT_MISSING",
      detail: loginErrorCopy("CLAUDE_LOGIN_FAILED"),
    };
  }
  try {
    const payload = await runLoginCommand<ClaudeLoginStartResult>(CLAUDE_COMMANDS.loginStart, sshPayload(input));
    const errorCode = payload.ok ? payload.errorCode : (payload.errorCode ?? "CLAUDE_LOGIN_FAILED");
    return {
      ok: payload.ok,
      loginUrl: payload.loginUrl ?? null,
      errorCode,
      detail: payload.detail ?? (errorCode ? loginErrorCopy(errorCode) : null),
    };
  } catch (error: unknown) {
    const errorCode = error instanceof LumioCommandError ? error.errorCode : "CLAUDE_LOGIN_FAILED";
    return { ok: false, loginUrl: null, errorCode, detail: loginErrorCopy(errorCode) };
  }
}

export async function submitClaudeLogin(
  input: ClaudeSshArgs & { code: string },
): Promise<ClaudeLoginSubmitResult> {
  if (!isTauri()) {
    return {
      ok: false,
      phase: "fail",
      errorCode: "SSH_CLIENT_MISSING",
      detail: loginErrorCopy("CLAUDE_LOGIN_FAILED"),
    };
  }
  try {
    const payload = await runLoginCommand<ClaudeLoginSubmitResult>(CLAUDE_COMMANDS.loginSubmit, {
      ...sshPayload(input),
      code: input.code,
    });
    const errorCode = payload.ok ? payload.errorCode : (payload.errorCode ?? "CLAUDE_LOGIN_CODE_REJECTED");
    return {
      ok: payload.ok,
      phase: payload.phase,
      errorCode,
      detail: payload.detail ?? (errorCode ? loginErrorCopy(errorCode) : null),
    };
  } catch (error: unknown) {
    const errorCode = error instanceof LumioCommandError ? error.errorCode : "CLAUDE_LOGIN_FAILED";
    return { ok: false, phase: "fail", errorCode, detail: loginErrorCopy(errorCode) };
  }
}

export async function loadClaudeLoginStatus(input: ClaudeSshArgs): Promise<ClaudeLoginStatusResult> {
  if (!isTauri()) {
    return { phase: "unknown", errorCode: "SSH_CLIENT_MISSING" };
  }
  try {
    return await runLoginCommand<ClaudeLoginStatusResult>(CLAUDE_COMMANDS.loginStatus, sshPayload(input));
  } catch (error: unknown) {
    const errorCode = error instanceof LumioCommandError ? error.errorCode : "CLAUDE_LOGIN_FAILED";
    return { phase: "unknown", errorCode };
  }
}

export function terminalOutputEvent(projectId: string, sessionId = DEFAULT_TERMINAL_SESSION_ID): string {
  return `lumio://claude-terminal-output-${projectId}-${sessionId}`;
}

export function terminalClosedEvent(projectId: string, sessionId = DEFAULT_TERMINAL_SESSION_ID): string {
  return `lumio://claude-terminal-closed-${projectId}-${sessionId}`;
}

export async function subscribeClaudeEvent<T>(
  event: string,
  handler: (payload: T) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return () => undefined;
  return listen<T>(event, (incoming) => handler(incoming.payload));
}
