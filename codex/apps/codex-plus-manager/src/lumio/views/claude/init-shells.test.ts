import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

async function readOwned(): Promise<{ init: string; login: string; initCss: string; loginCss: string }> {
  const dir = new URL("./", import.meta.url);
  const [init, login, initCss, loginCss] = await Promise.all([
    readFile(new URL("InitChecklist.tsx", dir), "utf8"),
    readFile(new URL("LoginCard.tsx", dir), "utf8"),
    readFile(new URL("InitChecklist.css", dir), "utf8"),
    readFile(new URL("LoginCard.css", dir), "utf8"),
  ]);
  return { init, login, initCss, loginCss };
}

test("the init checklist lists the five scheme D steps with retry callbacks", async () => {
  const { init } = await readOwned();
  assert.match(init, /连接服务器/);
  assert.match(init, /装同步组件/);
  assert.match(init, /首次同步文件/);
  assert.match(init, /安装 Claude/);
  assert.match(init, /登录 Anthropic/);
  assert.match(init, /正在准备…/);
  assert.match(init, /正在安装…/);
  assert.match(init, /重试这一步/);
  assert.match(init, /onRetryInstall/);
  assert.match(init, /onRetrySync/);
  assert.match(init, /CLAUDE_CLI_DOWNLOAD_FAILED/);
  assert.match(init, /服务器连不上官方下载地址/);
  assert.match(init, /failDetail/);
  assert.match(init, /errorCode/);
  assert.doesNotMatch(init, /安装失败/);
});

test("the login overlay does not invent a Claude version number", async () => {
  const { login } = await readOwned();
  assert.doesNotMatch(login, /2\.1\.228/);
  assert.match(login, /已装好/);
});

test("the login card pastes an authorization code in both layouts", async () => {
  const { init, login } = await readOwned();
  assert.match(login, /在浏览器中登录/);
  assert.match(login, /复制登录链接/);
  assert.match(login, /浏览器给了授权码？贴在这里/);
  assert.match(login, /placeholder="粘贴授权码"/);
  assert.match(login, /完成登录/);
  assert.match(login, /不用在黑底窗口里复制几十行地址、再对着提示盲贴。/);
  assert.match(login, /loginUrl: string \| null/);
  assert.match(login, /onOpenBrowser/);
  assert.match(login, /onCopyLink/);
  assert.match(login, /onSubmitCode/);
  assert.match(login, /登录已过期/);
  assert.match(login, /重新授权一次/);
  assert.match(login, /embedded/);
  assert.match(login, /overlay/);
  assert.match(init, /<LoginCard/);
  assert.match(init, /layout="embedded"/);
});

test("resume, offline and pick-project shells copy the prototype wording", async () => {
  const { init } = await readOwned();
  assert.match(init, /export function ResumeProgress/);
  assert.match(init, /连上服务器/);
  assert.match(init, /恢复上次的对话/);
  assert.match(init, /对齐文件/);
  assert.match(init, /不用你再填什么，几秒就好。/);
  assert.match(init, /进去看看/);
  assert.match(init, /export function OfflineCard/);
  assert.match(init, /连不上这台服务器/);
  assert.match(init, /照常能改/);
  assert.match(init, /不会静默覆盖谁/);
  assert.match(init, /看本机文件/);
  assert.match(init, /重试连接/);
  assert.match(init, /CLAUDE_SSH_TIMEOUT/);
  assert.match(init, /连接超时。检查这台服务器是否开机、网络是否可达。/);
  assert.match(init, /export function PickProjectHint/);
  assert.match(init, /挑一个项目/);
  assert.match(init, /左边点一下就自动连上那台服务器，上次的对话还在。/);
  assert.match(init, /每个项目就是一台服务器加一个文件夹。/);
});

test("init and login shells never mention agent, tmux, or the fake terminal failure", async () => {
  const { init, login, initCss, loginCss } = await readOwned();
  const source = [init, login, initCss, loginCss].join("\n");
  assert.doesNotMatch(source, /\bagent\b/i);
  assert.doesNotMatch(source, /\btmux\b/i);
  assert.doesNotMatch(source, /没能打开终端/);
  assert.doesNotMatch(init, /invoke\s*[<(]/);
  assert.doesNotMatch(login, /invoke\s*[<(]/);
  assert.doesNotMatch(source, /@tauri-apps\/api/);
  assert.doesNotMatch(source, /lumio_claude_/);
});

test("the fail code is monospace and the login overlay sits above the terminal", async () => {
  const { initCss, loginCss } = await readOwned();
  assert.match(initCss, /ui-monospace/);
  assert.match(loginCss, /is-overlay/);
  assert.match(loginCss, /backdrop-filter/);
});
