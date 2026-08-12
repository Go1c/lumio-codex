import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
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

// `provider` 不进这条列表：React 的 `Context.Provider` 是框架 API 而非产品用语。
// 面向用户的 `provider` 红线由上面的 shellLabels 与 errors.ts 两条测试守住。
const FORBIDDEN_SURFACES = ["base url", "stepwise", "dream skin", "api key", "mcp", "plugin"];

// import 语句里的包名与绑定标识符是依赖名而不是产品文案（如 `@tauri-apps/plugin-dialog`），
// 屏幕上永远看不到，扫描前先摘掉，免得红线退化成给依赖改名。
function userFacingSource(source: string): string {
  return source.replace(/^import\s(?:[\s\S]*?from\s+)?"[^"]+";$/gm, "");
}

test("no user-facing source file mentions a forbidden product surface", async () => {
  const viewsDir = new URL("./views/", import.meta.url);
  const views = (await readdir(viewsDir)).filter((name) => name.endsWith(".tsx")).sort();

  // 视图清单写死：改名或删文件都会在这里暴露，扫描覆盖面不会静默缩小。
  assert.deepEqual(views, [
    "HomeView.tsx",
    "LoginView.tsx",
    "ProvisioningView.tsx",
    "RegisterView.tsx",
    "RepairView.tsx",
    "SettingsView.tsx",
    "SignedOutView.tsx",
    "Toast.tsx",
  ]);

  const sources: [string, string][] = [
    ["LumioApp.tsx", await readFile(new URL("../LumioApp.tsx", import.meta.url), "utf8")],
    ...(await Promise.all(
      views.map(
        async (name): Promise<[string, string]> => [
          `views/${name}`,
          await readFile(new URL(name, viewsDir), "utf8"),
        ],
      ),
    )),
  ];

  for (const [name, source] of sources) {
    const lowered = userFacingSource(source).toLowerCase();
    for (const forbidden of FORBIDDEN_SURFACES) {
      assert.equal(
        lowered.includes(forbidden),
        false,
        `forbidden term "${forbidden}" in ${name}`,
      );
    }
  }
});

test("the forbidden-surface scan still reads the copy around an import statement", () => {
  const source = [
    'import { open } from "@tauri-apps/plugin-dialog";',
    'import {',
    '  useState,',
    '} from "react";',
    'const label = "填写 API Key";',
  ].join("\n");

  assert.equal(userFacingSource(source).includes("plugin-dialog"), false);
  assert.equal(userFacingSource(source).includes("useState"), false);
  assert.ok(userFacingSource(source).includes("API Key"));
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

test("the register view carries the spec copy for every state", async () => {
  const view = await readFile(new URL("./views/RegisterView.tsx", import.meta.url), "utf8");

  assert.match(view, /重新发送/);
  assert.match(view, /两次输入不一致/);
  assert.match(view, /密码至少 8 位/);
  assert.match(view, /正在创建账户…/);
  assert.match(view, /已有账户？去登录/);
  assert.match(view, /注册暂未开放/);
  assert.match(view, /返回登录/);
});

test("the login view carries the spec copy including the two-factor step", async () => {
  const view = await readFile(new URL("./views/LoginView.tsx", import.meta.url), "utf8");

  assert.match(view, /正在验证…/);
  assert.match(view, /忘记密码？/);
  assert.match(view, /密码重置在网页端完成/);
  assert.match(view, /在浏览器中打开/);
  assert.match(view, /输入两步验证码/);
  assert.match(view, /打开你的验证器应用查看动态码/);
  assert.match(view, /返回重新登录/);
  assert.match(view, /没有账户？创建账户/);
});

test("neither auth view hardcodes business rules the server owns", async () => {
  const register = await readFile(new URL("./views/RegisterView.tsx", import.meta.url), "utf8");

  assert.match(register, /settings\.emailSuffixWhitelist|formatEmailSuffixHint/);
  assert.match(register, /settings\.agreementDocuments/);
  assert.doesNotMatch(register, /@gmail\.com|@qq\.com/);
});

test("the provisioning view carries the spec copy for slow and failed steps", async () => {
  const view = await readFile(new URL("./views/ProvisioningView.tsx", import.meta.url), "utf8");

  assert.match(view, /不需要手动操作，完成后自动进入首页/);
  assert.match(view, /比平时慢一些，仍在继续…/);
  assert.match(view, /重试/);
  assert.match(view, /稍后处理/);
  assert.match(view, /PROVISIONING_STEP_TITLES/);
  assert.doesNotMatch(view, /返回/, "the provisioning page must not offer a back exit");
});

test("the home view explains every disabled action instead of hiding it", async () => {
  const view = await readFile(new URL("./views/HomeView.tsx", import.meta.url), "utf8");

  assert.match(view, /actionNotes\.launch/);
  assert.match(view, /actionNotes\.pay/);
  assert.match(view, /actionNotes\.refresh/);
  assert.match(view, /官方 Codex 已启动/);
  assert.match(view, /刷新失败，仍显示上次数据/);
  assert.match(view, /无法连接服务，正在使用/);
  assert.match(view, /你仍可以启动官方 Codex。/);
  assert.match(view, /已重新连接/);
  assert.match(view, /缓存值/);
  assert.match(view, /已配置/);
});

test("the repair view offers the three spec actions and never a force overwrite", async () => {
  const view = await readFile(new URL("./views/RepairView.tsx", import.meta.url), "utf8");

  assert.match(view, /需要检查配置/);
  assert.match(view, /重新检查/);
  assert.match(view, /恢复本机配置/);
  assert.match(view, /将撤销由 Lumio 管理的配置字段并恢复接管前内容，你的其他本机设置不受影响。/);
  assert.match(view, /导出诊断日志/);
  assert.match(view, /导出前会再次扫描并移除敏感内容/);
  assert.match(view, /问题仍未解决/);
  assert.match(view, /本机保存的登录凭据已失效，请重新登录/);
  assert.doesNotMatch(view, /强制覆盖/);
});

test("the settings view keeps every approved row and its explanation", async () => {
  const view = await readFile(new URL("./views/SettingsView.tsx", import.meta.url), "utf8");

  assert.match(view, /你仍会收到重要安全更新提示/);
  assert.match(view, /重新检测/);
  assert.match(view, /手动选择…/);
  assert.match(view, /未检测到，可手动选择/);
  assert.match(view, /所选应用无法识别为官方 Codex/);
  assert.match(view, /不可用的选项会保持禁用，不会修改本机配置。/);
});

test("enabling telemetry requires an explicit confirmation that lists the collected fields", async () => {
  const view = await readFile(new URL("./views/SettingsView.tsx", import.meta.url), "utf8");

  assert.match(view, /版本/);
  assert.match(view, /平台/);
  assert.match(view, /阶段/);
  assert.match(view, /脱敏错误码/);
});

test("React entry renders only LumioApp", async () => {
  const main = await readFile(new URL("../main.tsx", import.meta.url), "utf8");

  assert.match(main, /import \{ LumioApp \} from "\.\/LumioApp"/);
  assert.doesNotMatch(main, /from "\.\/App"/);
});
