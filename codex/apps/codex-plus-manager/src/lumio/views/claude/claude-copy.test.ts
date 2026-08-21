import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import test from "node:test";

import { readAllClaudeViews } from "../../claude/read-claude-views.ts";

async function readView(name: string): Promise<string> {
  return readFile(new URL(name, import.meta.url), "utf8");
}

test("the Claude workspace folder ships the four prototype surfaces", async () => {
  const names = (await readdir(new URL(".", import.meta.url)))
    .filter((name) => name.endsWith(".tsx"))
    .sort();
  for (const required of [
    "ClaudeConnect.tsx",
    "ClaudeEmpty.tsx",
    "ClaudeEntitlementLine.tsx",
    "ClaudeHome.tsx",
    "ClaudeSubscribe.tsx",
    "ClaudeWorkspace.tsx",
    "ProjectRail.tsx",
    "TerminalPane.tsx",
    "FileExplorer.tsx",
    "ConflictsPane.tsx",
    "StatusDrawer.tsx",
    "StatusBar.tsx",
    "SessionTabs.tsx",
    "InitChecklist.tsx",
    "LoginCard.tsx",
  ]) {
    assert.ok(names.includes(required), `missing ${required}`);
  }
});

test("the subscribe card uses the plan price and pays with account balance", async () => {
  const source = await readView("ClaudeSubscribe.tsx");
  const shell = await readFile(new URL("../../../LumioApp.tsx", import.meta.url), "utf8");
  const session = await readFile(new URL("../../claude/session.ts", import.meta.url), "utf8");
  const types = await readFile(new URL("../../claude/types.ts", import.meta.url), "utf8");
  assert.match(source, /在自己的服务器上/);
  assert.match(source, /\/ 月/);
  assert.match(source, /独立环境、双向同步、不限项目/);
  assert.match(source, /用余额支付/);
  assert.match(source, /去充值/);
  assert.match(source, /正在支付…/);
  assert.match(source, /余额 ¥/);
  assert.match(source, /onPay/);
  assert.match(source, /onRecharge/);
  assert.match(source, /回到 Codex Tab/);
  assert.doesNotMatch(source, /onOpenAccount/);
  assert.doesNotMatch(source, /CLAUDE_ACCOUNT_URL/);
  assert.doesNotMatch(source, /CLAUDE_ORDERS_URL/);
  // 主按钮是余额支付，不能把 purchase 当开通主路径；充值回调在壳里打开 paymentUrl。
  assert.doesNotMatch(source, /purchase/);
  assert.doesNotMatch(shell, /openInBrowser\(CLAUDE_ACCOUNT_URL\)/);
  assert.doesNotMatch(shell, /openClaudeSubscribe/);
  assert.match(shell, /paymentUrl/);
  assert.doesNotMatch(shell, /onOpenAccount=\{openSettings\}/);
  assert.match(shell, /onOpenOrders=\{openClaudeOrders\}/);
  assert.match(shell, /openInBrowser\(CLAUDE_ORDERS_URL\)/);
  assert.match(session, /DEFAULT_CLAUDE_PLAN_CENTS/);
  assert.match(types, /1990/);
  assert.doesNotMatch(types, /6800/);
  assert.doesNotMatch(session, /6800/);
});

test("the first-run page lists the three promises before any SSH form", async () => {
  const source = await readView("ClaudeEmpty.tsx");
  assert.match(source, /独立环境/);
  assert.match(source, /本机仍能改文件/);
  assert.match(source, /一次登录/);
  assert.match(source, /连接一台服务器/);
  assert.match(source, /先留在 Codex/);
  assert.doesNotMatch(source, /type="password"/);
});

test("the connect sheet has the four prototype steps and the SSH paste hint", async () => {
  const source = await readView("ClaudeConnect.tsx");
  assert.match(source, />主机</);
  assert.match(source, />探测</);
  assert.match(source, />装组件</);
  assert.match(source, />首次同步</);
  assert.match(source, /探测连接/);
  assert.match(source, /正在探测…/);
  assert.match(source, /lumio-button-spinner/);
  assert.match(source, /aria-busy=\{probing\}/);
  const css = await readFile(new URL("../../../lumio-shell.css", import.meta.url), "utf8");
  assert.match(css, /\.lumio-button-spinner/);
  assert.match(css, /\.lumio-button\.is-busy/);
  assert.match(source, /本机 SSH 方式/);
  assert.match(source, /IP 用户密码/);
  assert.match(source, /主机IP/);
  assert.match(source, /ssh root@/);
  assert.match(source, /密码只留在这台电脑上/);
  assert.match(source, /sheet\.mode === "project"/);
  assert.match(source, /在这台服务器上再建一个项目/);
  assert.match(source, /SSH_AUTH_FAILED/);
  assert.match(source, /IP 是否抄对/);
  assert.match(source, /密码是否正确/);
  assert.match(source, /安全组是否放行 22/);
  const paths = await readFile(new URL("../../claude/paths.ts", import.meta.url), "utf8");
  assert.match(paths, /~\/bestcodex\//);
  assert.match(paths, /~\/BestCodex\//);
  assert.match(source, /SSH 配置|Host 别名|配置别名/);
  assert.match(source, /setupStatus === "fail"/);
  assert.match(source, /服务器上已有这个项目/);
  assert.match(source, /继续使用/);
  assert.match(source, /新建 /);
  assert.match(source, /已经装过同步组件/);
  assert.match(source, /setupStatus === "reinstall"/);
  assert.match(source, />重装</);
  assert.match(source, /sync\.state === "fail"/);
  assert.match(source, /本机文件夹/);
  assert.match(source, /服务器文件夹/);
  assert.match(source, /选择本机文件夹/);
  assert.doesNotMatch(source, /当作完成/);
  assert.doesNotMatch(source, /懂 SSH 再用/);
});

test("user-visible Claude copy never says agent or tmux", async () => {
  const names = (await readdir(new URL(".", import.meta.url))).filter(
    (name) => (name.endsWith(".tsx") || name.endsWith(".ts")) && !name.endsWith(".test.ts"),
  );
  assert.ok(names.length > 0);
  for (const name of names) {
    const source = await readView(name);
    assert.doesNotMatch(source, /\bagent\b/i, `${name} leaked agent`);
    assert.doesNotMatch(source, /\btmux\b/i, `${name} leaked tmux`);
  }
  for (const rel of [
    "../../claude/session.ts",
    "../../claude/api.ts",
    "../../claude/machine.ts",
    "../../claude/terminal-status.ts",
  ]) {
    const source = await readFile(new URL(rel, import.meta.url), "utf8");
    const visible = source
      .split("\n")
      .filter((line) => !line.trimStart().startsWith("//") && !line.trimStart().startsWith("*"))
      .filter((line) => /["'`]/.test(line))
      .join("\n");
    assert.doesNotMatch(visible, /\bagent\b/i, `${rel} leaked agent`);
    assert.doesNotMatch(visible, /\btmux\b/i, `${rel} leaked tmux`);
  }
});

test("the terminal copies locally and opens login links in the system browser", async () => {
  const views = await readAllClaudeViews();
  const logic = await readFile(new URL("../../claude/terminal-clipboard.ts", import.meta.url), "utf8");
  assert.match(views, /onContextMenu/);
  assert.match(views, /复制/);
  assert.match(views, /用浏览器打开/);
  assert.match(views, /openInBrowser/);
  assert.match(views, /attachCustomKeyEventHandler/);
  assert.match(views, /copyTextForKey/);
  assert.match(logic, /stitchWrappedHttpsUrls/);
  assert.doesNotMatch(views, /window\.open/);
});

test("the workspace shows files and conflicts tabs next to the terminal", async () => {
  const source = await readAllClaudeViews();
  const css = await readFile(new URL("./claude-workspace.css", import.meta.url), "utf8");
  const home = await readView("ClaudeHome.tsx");
  assert.match(home, /beginNewProjectOnHost/);
  assert.match(home, /ProjectRail/);
  assert.match(home, /SessionTabs/);
  assert.match(home, /FileExplorer/);
  assert.match(home, /StatusBar/);
  assert.match(home, /StatusDrawer/);
  assert.match(source, /服务器与项目/);
  assert.match(source, /新建项目/);
  assert.match(source, /连接新服务器/);
  assert.match(css, /grid-template-columns:\s*236px minmax\(0,\s*1fr\) 282px/);
  assert.match(css, /grid-template-rows:\s*minmax\(0,\s*1fr\) 26px/);
  assert.match(css, /\.lumio-claude-sheet-back\s*\{[^}]*position:\s*fixed/);
  assert.doesNotMatch(home, /lumio-claude-stage-tabs/);
  assert.doesNotMatch(home, /set-stage-tab/);
  assert.doesNotMatch(home, /ClaudeEntitlementLine/);
  assert.doesNotMatch(home, /ordersSlot/);
});

test("session title locking and last-tab refill are wired from the workspace", async () => {
  const home = await readView("ClaudeHome.tsx");
  const terminal = await readView("TerminalPane.tsx");
  assert.match(terminal, /lockTitleFromInput/);
  assert.match(terminal, /session-title-locked/);
  assert.match(terminal, /sessionId/);
  assert.match(home, /nextSessionId/);
  assert.match(home, /close-session/);
  assert.match(home, /open-session/);
  assert.match(home, /lumio_claude_close_chat|closeClaudeProjectChat/);
});

test("the terminal viewport is clipped to the pane and refits on resize", async () => {
  const source = await readView("TerminalPane.tsx");
  const css = await readFile(new URL("./TerminalPane.css", import.meta.url), "utf8");
  assert.match(source, /ResizeObserver/);
  assert.match(css, /\.lumio-claude-xterm-wrap\s*\{[^}]*overflow:\s*hidden/);
  assert.match(css, /\.lumio-claude-xterm\s*\{[^}]*overflow:\s*hidden/);
});

test("terminalBanner and terminal output are mutually exclusive", async () => {
  const terminal = await readView("TerminalPane.tsx");
  const status = await readFile(new URL("../../claude/terminal-status.ts", import.meta.url), "utf8");
  assert.match(terminal, /terminalBanner/);
  assert.match(terminal, /setHasOutput\(true\)/);
  assert.match(status, /没能打开终端/);
  assert.doesNotMatch(terminal, /setStatus\("没能打开终端。"\)/);
});

test("ClaudeWorkspace keeps session state in the module store, not only in the leaf", async () => {
  const source = await readView("ClaudeWorkspace.tsx");
  const views = await readAllClaudeViews();
  assert.match(source, /getClaudeState|subscribeClaudeStore/);
  assert.match(source, /onBackToCodex/);
  assert.match(source, /onRecharge/);
  assert.match(source, /onOpenOrders/);
  assert.doesNotMatch(source, /onOpenAccount/);
  assert.match(views, /开通记录|ordersSlot/);
  assert.doesNotMatch(source, /toggleClaudeOrders/);
  assert.doesNotMatch(source, /暂无开通记录/);
});

test("开通记录 opens the account-center orders tab", async () => {
  const portal = await readFile(new URL("../../claude/portal.ts", import.meta.url), "utf8");
  assert.match(portal, /export const CLAUDE_ORDERS_URL/);
  assert.match(portal, /https:\/\/bestcodex\.app\/account#orders/);
});

test("empty and home surfaces show remaining subscription days from the server", async () => {
  const line = await readView("ClaudeEntitlementLine.tsx");
  const views = await readAllClaudeViews();
  const copy = await readFile(new URL("../../claude/copy.ts", import.meta.url), "utf8");
  assert.match(copy, /有效期至/);
  assert.match(copy, /剩余/);
  assert.match(views, /ClaudeEntitlementLine/);
  assert.match(line, /即将到期/);
});

test("setup shows live progress instead of a silent automatic step", async () => {
  const connect = await readView("ClaudeConnect.tsx");
  const api = await readFile(new URL("../../claude/api.ts", import.meta.url), "utf8");
  const session = await readFile(new URL("../../claude/session.ts", import.meta.url), "utf8");
  assert.match(connect, /setupProgress/);
  assert.match(connect, /lumio-claude-progress/);
  assert.match(connect, /formatSetupElapsed/);
  assert.match(connect, /正在安装…/);
  assert.doesNotMatch(connect, /自动完成，无需操作/);
  assert.match(api, /CLAUDE_PREPARE_PROGRESS_EVENT/);
  assert.match(api, /已用/);
  assert.match(api, /正在检查服务器/);
  assert.match(api, /正在把同步组件传到服务器/);
  assert.match(session, /CLAUDE_PREPARE_PROGRESS_EVENT/);
  assert.match(session, /setup-progress/);
  assert.match(session, /ensureClaudeEngineBridge/);
});

test("artifact-missing blames the build, not the machine or the connection", async () => {
  const connect = await readView("ClaudeConnect.tsx");
  const api = await readFile(new URL("../../claude/api.ts", import.meta.url), "utf8");
  assert.match(api, /这个版本的 BestCodex 没有把同步组件打进来/);
  assert.doesNotMatch(api, /这台电脑还没有同步组件/);
  // DEPLOY_ARTIFACT_MISSING 有专属失败面：不引导改连接，给帮助页 + 重试
  assert.match(connect, /setupErrorCode === "DEPLOY_ARTIFACT_MISSING"/);
  assert.match(connect, /打开帮助页/);
  assert.match(connect, /HELP_URL/);
});

test("ClaudeHome maps TerminalPane from sessionsByProject, not only the active sessions alias", async () => {
  const home = await readView("ClaudeHome.tsx");
  assert.match(home, /Object\.entries\(\s*state\.sessionsByProject\s*\)/);
  assert.match(home, /<TerminalPane/);
  assert.doesNotMatch(home, /sessions\.map\(\(session\) =>\s*\(\s*<TerminalPane/);
});

test("ClaudeHome passes onlineHosts into ProjectRail from state", async () => {
  const home = await readView("ClaudeHome.tsx");
  assert.match(home, /onlineHosts=/);
  assert.match(home, /onlineHostsFromState/);
});

test("the offline card can be dismissed back to the workspace", async () => {
  const home = await readView("ClaudeHome.tsx");
  assert.match(home, /<OfflineCard/);
  assert.match(home, /onDismiss=/);
  assert.match(
    home,
    /onDismiss=\{\(\) => \{[\s\S]*set-workspace-phase[\s\S]*phase: "ready"/,
  );
});
