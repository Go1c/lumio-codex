import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
  CLAUDE_CLI_PROGRESS_EVENT,
  CLAUDE_LOGIN_PROGRESS_EVENT,
  cliErrorCopy,
  localFsErrorCopy,
  loginErrorCopy,
  terminalClosedEvent,
  terminalOutputEvent,
} from "./api.ts";

test("session-scoped terminal events include project and session ids", () => {
  assert.equal(terminalOutputEvent("proj", "chat-2"), "lumio://claude-terminal-output-proj-chat-2");
  assert.equal(terminalClosedEvent("proj", "chat-2"), "lumio://claude-terminal-closed-proj-chat-2");
  assert.equal(terminalOutputEvent("proj"), "lumio://claude-terminal-output-proj-default");
});

test("CLI and login progress events match the backend names", () => {
  assert.equal(CLAUDE_CLI_PROGRESS_EVENT, "lumio://claude-cli-progress");
  assert.equal(CLAUDE_LOGIN_PROGRESS_EVENT, "lumio://claude-login-progress");
});

test("CLAUDE_CLI_* codes have human copy and never mention agent or tmux", () => {
  assert.equal(cliErrorCopy("CLAUDE_CLI_NO_NETWORK"), "这台服务器现在连不上外网。检查网络后再试。");
  assert.equal(cliErrorCopy("CLAUDE_CLI_DNS"), "这台服务器解析不了官方下载地址。检查 DNS 后再试。");
  assert.equal(cliErrorCopy("CLAUDE_CLI_NO_CURL"), "这台服务器没有 curl，装不上官方 Claude。");
  assert.equal(cliErrorCopy("CLAUDE_CLI_BIN_UNWRITABLE"), "写不进 ~/.local/bin，没法安装 Claude。");
  assert.equal(
    cliErrorCopy("CLAUDE_CLI_DOWNLOAD_FAILED"),
    "服务器连不上官方下载地址。确认这台服务器能访问外网，或稍后再试。",
  );
  assert.equal(cliErrorCopy("CLAUDE_CLI_VERIFY_FAILED"), "装完之后没能读到 Claude 版本，安装可能没有成功。");
  assert.equal(cliErrorCopy("CLAUDE_CLI_INSTALL_FAILED"), "没能在这台服务器上装好 Claude。");
  for (const code of [
    "CLAUDE_CLI_NO_NETWORK",
    "CLAUDE_CLI_DNS",
    "CLAUDE_CLI_NO_CURL",
    "CLAUDE_CLI_BIN_UNWRITABLE",
    "CLAUDE_CLI_DOWNLOAD_FAILED",
    "CLAUDE_CLI_VERIFY_FAILED",
    "CLAUDE_CLI_INSTALL_FAILED",
    "UNKNOWN",
  ]) {
    const copy = cliErrorCopy(code);
    assert.doesNotMatch(copy, /\bagent\b/i);
    assert.doesNotMatch(copy, /\btmux\b/i);
  }
});

test("CLAUDE_LOGIN_* codes have human copy and never mention agent or tmux", () => {
  assert.equal(loginErrorCopy("CLAUDE_LOGIN_NO_CLI"), "服务器上还没有官方 Claude 命令。");
  assert.equal(loginErrorCopy("CLAUDE_LOGIN_NO_URL"), "没能拿到登录链接。");
  assert.equal(loginErrorCopy("CLAUDE_LOGIN_CODE_REJECTED"), "授权码未被接受。");
  assert.equal(loginErrorCopy("CLAUDE_LOGIN_EXPIRED"), "登录已过期。");
  assert.equal(loginErrorCopy("CLAUDE_LOGIN_FAILED"), "没能完成 Anthropic 登录。");
  for (const code of [
    "CLAUDE_LOGIN_NO_CLI",
    "CLAUDE_LOGIN_NO_URL",
    "CLAUDE_LOGIN_CODE_REJECTED",
    "CLAUDE_LOGIN_EXPIRED",
    "CLAUDE_LOGIN_FAILED",
    "UNKNOWN",
  ]) {
    const copy = loginErrorCopy(code);
    assert.doesNotMatch(copy, /\bagent\b/i);
    assert.doesNotMatch(copy, /\btmux\b/i);
  }
});

test("api.ts registers install, login, and chat commands", async () => {
  const source = await readFile(new URL("./api.ts", import.meta.url), "utf8");
  assert.match(source, /lumio_claude_install_cli/);
  assert.match(source, /lumio_claude_login_start/);
  assert.match(source, /lumio_claude_login_submit/);
  assert.match(source, /lumio_claude_login_status/);
  assert.match(source, /lumio_claude_open_chat/);
  assert.match(source, /lumio_claude_close_chat/);
  assert.match(source, /lumio_claude_list_chats/);
  assert.match(source, /lumio_claude_local_fs/);
  assert.match(source, /sessionId/);
  assert.doesNotMatch(source, /\bagent\b/i);
  assert.doesNotMatch(source, /\btmux\b/i);
});

test("localFsErrorCopy maps file command codes to human copy", () => {
  assert.equal(localFsErrorCopy("FILE_EXISTS"), "已经有这个名字。");
  assert.equal(localFsErrorCopy("FILE_MISSING"), "找不到这个文件。");
  assert.equal(localFsErrorCopy("FILE_NAME_INVALID"), "这个名字不能用。");
  assert.equal(localFsErrorCopy("FILE_REVEAL_FAILED"), "没能在 Finder 里打开。");
  assert.equal(localFsErrorCopy("PATH_OUTSIDE_PROJECT"), "路径必须位于项目文件夹内。");
  assert.equal(localFsErrorCopy("FILE_WRITE_FAILED"), "没能改这个文件。");
  assert.equal(localFsErrorCopy("UNKNOWN"), "没能改这个文件。");
  assert.equal(localFsErrorCopy("路径必须位于项目文件夹内。"), "路径必须位于项目文件夹内。");
});
