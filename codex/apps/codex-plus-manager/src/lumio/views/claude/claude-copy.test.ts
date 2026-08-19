import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import test from "node:test";

async function readView(name: string): Promise<string> {
  return readFile(new URL(name, import.meta.url), "utf8");
}

test("the Claude workspace folder ships the four prototype surfaces", async () => {
  const names = (await readdir(new URL(".", import.meta.url)))
    .filter((name) => name.endsWith(".tsx"))
    .sort();
  assert.deepEqual(names, [
    "ClaudeConnect.tsx",
    "ClaudeEmpty.tsx",
    "ClaudeEntitlementLine.tsx",
    "ClaudeHome.tsx",
    "ClaudeSubscribe.tsx",
    "ClaudeWorkspace.tsx",
  ]);
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
  // 主按钮是余额支付，不能把 purchase 当开通主路径；充值回调在壳里打开 paymentUrl。
  assert.doesNotMatch(source, /purchase/);
  assert.doesNotMatch(shell, /openInBrowser\(CLAUDE_ACCOUNT_URL\)/);
  assert.doesNotMatch(shell, /openClaudeSubscribe/);
  assert.match(shell, /paymentUrl/);
  assert.doesNotMatch(shell, /onOpenAccount=\{openSettings\}/);
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
  assert.match(source, /懂 SSH 再用/);
  assert.match(source, /ssh root@/);
  assert.match(source, /密码只留在这台电脑上/);
  assert.match(source, /SSH_AUTH_FAILED/);
  assert.match(source, /IP 是否抄对/);
  assert.match(source, /密码是否正确/);
  assert.match(source, /安全组是否放行 22/);
  assert.match(source, /\/root\/bestcodex\//);
  assert.match(source, /~\/BestCodex\//);
  assert.match(source, /SSH 配置|Host 别名|配置别名/);
  assert.match(source, /setupStatus === "fail"/);
  assert.match(source, /sync\.state === "fail"/);
  assert.doesNotMatch(source, /当作完成/);
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
  for (const rel of ["../../claude/session.ts", "../../claude/api.ts", "../../claude/machine.ts"]) {
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

test("the workspace shows files and conflicts tabs next to the terminal", async () => {
  const source = await readView("ClaudeHome.tsx");
  assert.match(source, />终端</);
  assert.match(source, />文件</);
  assert.match(source, />冲突</);
  assert.match(source, /连接新服务器/);
  assert.match(source, />项目</);
});

test("ClaudeWorkspace keeps session state in the module store, not only in the leaf", async () => {
  const source = await readView("ClaudeWorkspace.tsx");
  const empty = await readView("ClaudeEmpty.tsx");
  const home = await readView("ClaudeHome.tsx");
  const subscribe = await readView("ClaudeSubscribe.tsx");
  assert.match(source, /getClaudeState|subscribeClaudeStore/);
  assert.match(source, /onBackToCodex/);
  assert.match(source, /onRecharge/);
  assert.doesNotMatch(source, /onOpenAccount/);
  assert.match(`${source}\n${empty}\n${home}\n${subscribe}`, /开通记录|ordersSlot/);
  assert.match(source, /处理中，请勿重复支付/);
});

test("empty and home surfaces show remaining subscription days from the server", async () => {
  const line = await readView("ClaudeEntitlementLine.tsx");
  const empty = await readView("ClaudeEmpty.tsx");
  const home = await readView("ClaudeHome.tsx");
  const copy = await readFile(new URL("../../claude/copy.ts", import.meta.url), "utf8");
  assert.match(copy, /有效期至/);
  assert.match(copy, /剩余/);
  assert.match(`${empty}\n${home}`, /ClaudeEntitlementLine/);
  assert.match(line, /即将到期/);
});
