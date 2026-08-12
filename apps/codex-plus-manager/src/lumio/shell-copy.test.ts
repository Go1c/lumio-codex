import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { LUMIO_COMMANDS, LumioCommandError, visibleShellLabels } from "./invoke.ts";

test("the shell binds exactly the lumio command surface", () => {
  assert.deepEqual(LUMIO_COMMANDS, {
    bootstrap: "lumio_bootstrap",
    publicSettings: "lumio_public_settings",
    sendVerifyCode: "lumio_send_verify_code",
    register: "lumio_register",
    login: "lumio_login",
    loginTwoFactor: "lumio_login_two_factor",
    logout: "lumio_logout",
    refreshAccount: "lumio_refresh_account",
    provisionStep: "lumio_provision_step",
    takeoverHealth: "lumio_takeover_health",
    restoreConfig: "lumio_restore_config",
    launchCodex: "lumio_launch_codex",
    detectCodexApp: "lumio_detect_codex_app",
    selectCodexApp: "lumio_select_codex_app",
    openBrowser: "lumio_open_browser",
    setTelemetry: "lumio_set_telemetry",
    exportLogs: "lumio_export_logs",
  });
});

test("every bound command carries the lumio prefix", () => {
  for (const command of Object.values(LUMIO_COMMANDS)) {
    assert.ok(command.startsWith("lumio_"), `unexpected command: ${command}`);
  }
});

test("shell label inventory covers the approved product surface", () => {
  assert.deepEqual(visibleShellLabels, [
    "账户状态",
    "余额与套餐",
    "连接状态",
    "默认模型",
    "充值",
    "启动 Codex",
    "开机启动",
    "自动更新",
    "官方应用路径",
    "遥测",
    "日志导出",
    "配置恢复",
    "登录",
    "创建账户",
    "首页",
    "设置",
    "验证账户",
    "准备连接",
    "同步模型目录",
    "写入本机配置",
    "重新检查",
    "恢复本机配置",
    "导出诊断日志",
  ]);
});

test("a failed command surfaces its stable error code", () => {
  const error = new LumioCommandError("AUTH_INVALID_CREDENTIALS");

  assert.equal(error.errorCode, "AUTH_INVALID_CREDENTIALS");
  assert.equal(error.message, "AUTH_INVALID_CREDENTIALS");
  assert.ok(error instanceof Error);
});

test("shell copy excludes Codex++ enhancement surfaces", () => {
  const copy = visibleShellLabels.join(" ").toLowerCase();
  for (const forbidden of [
    "provider",
    "base url",
    "api key",
    "stepwise",
    "mcp",
    "plugin",
    "dream skin",
  ]) {
    assert.equal(copy.includes(forbidden), false);
  }
});

test("the shell no longer renders marketing-style brand decoration", async () => {
  const shell = await readFile(new URL("../LumioApp.tsx", import.meta.url), "utf8");
  const signedOut = await readFile(new URL("./views/SignedOutView.tsx", import.meta.url), "utf8");

  for (const source of [shell, signedOut]) {
    assert.doesNotMatch(source, /lumio-aurora/);
    assert.doesNotMatch(source, /lumio-orbit/);
  }
});

test("the removed decoration classes are gone from the stylesheet too", async () => {
  const css = await readFile(new URL("../lumio-shell.css", import.meta.url), "utf8");

  assert.doesNotMatch(css, /lumio-aurora/);
  assert.doesNotMatch(css, /lumio-orbit/);
});

test("the signed-out surface states the positioning promise verbatim", async () => {
  const signedOut = await readFile(new URL("./views/SignedOutView.tsx", import.meta.url), "utf8");

  assert.match(signedOut, /更快开始使用官方 Codex。/);
  assert.match(
    signedOut,
    /这个小工具只做一件事：帮你完成注册、登录和本机配置，省去手动安装配置的步骤。之后你使用的始终是官方 Codex 应用，一切保持原生。/,
  );
  assert.match(signedOut, /不修改官方应用/);
  assert.match(signedOut, /配置可一键恢复/);
  assert.match(signedOut, /凭据由系统保护/);
});

test("React entry renders only LumioApp", async () => {
  const main = await readFile(new URL("../main.tsx", import.meta.url), "utf8");

  assert.match(main, /import \{ LumioApp \} from "\.\/LumioApp"/);
  assert.doesNotMatch(main, /from "\.\/App"/);
});
