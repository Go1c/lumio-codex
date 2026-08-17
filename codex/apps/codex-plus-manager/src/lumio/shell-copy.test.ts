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
    checkUpdate: "lumio_check_update",
    downloadUpdate: "lumio_download_update",
    dismissUpdate: "lumio_dismiss_update",
    updateNoticeShown: "lumio_update_notice_shown",
    setTelemetry: "lumio_set_telemetry",
    setLaunchAtLogin: "lumio_set_launch_at_login",
    exportLogs: "lumio_export_logs",
    installOfficialApp: "lumio_install_official_app",
    officialAppStatus: "lumio_official_app_status",
    cancelOfficialApp: "lumio_cancel_official_app",
  });
});

test("every bound command carries the lumio prefix", () => {
  for (const command of Object.values(LUMIO_COMMANDS)) {
    assert.ok(command.startsWith("lumio_"), `unexpected command: ${command}`);
  }
});

test("shell label inventory covers the approved product surface", () => {
  assert.deepEqual(visibleShellLabels, [
    "账户",
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
    "Codex",
    "Claude",
    "设置",
    "通用",
    "支持",
    "帮助中心",
    "验证账户",
    "准备连接",
    "同步模型目录",
    "写入本机配置",
    "重新检查",
    "恢复本机配置",
    "导出诊断日志",
  ]);
  const labels: readonly string[] = visibleShellLabels;
  assert.equal(labels.includes("首页"), false);
  assert.equal(labels.includes("余额与套餐"), false);
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
  const topLevel = (await readdir(viewsDir)).filter((name) => name.endsWith(".tsx"));
  const claudeDir = new URL("./claude/", viewsDir);
  const claudeViews = (await readdir(claudeDir))
    .filter((name) => name.endsWith(".tsx"))
    .map((name) => `claude/${name}`);
  const views = [...topLevel, ...claudeViews].sort();

  // 必扫清单写死：改名或删文件都会在这里暴露。Claude 子目录里其他 agent 可增文件，那些也会被扫到。
  for (const required of [
    "HomeView.tsx",
    "LoginView.tsx",
    "ProvisioningView.tsx",
    "RegisterView.tsx",
    "RepairView.tsx",
    "SettingsView.tsx",
    "SignedOutView.tsx",
    "Toast.tsx",
    "claude/ClaudeWorkspace.tsx",
  ]) {
    assert.ok(views.includes(required), `missing required view ${required}`);
  }

  const sources: [string, string][] = [
    ["LumioApp.tsx", await readFile(new URL("../LumioApp.tsx", import.meta.url), "utf8")],
    // state.ts 里有会上屏的文案常量（禁用说明等），禁词扫描必须覆盖到。
    ["state.ts", await readFile(new URL("./state.ts", import.meta.url), "utf8")],
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

test("the signed-out surface uses the BestCodex welcome copy", async () => {
  const signedOut = await readFile(new URL("./views/SignedOutView.tsx", import.meta.url), "utf8");

  assert.match(signedOut, /\/lumio-icon\.png/);
  assert.match(signedOut, /BestCodex/);
  assert.match(signedOut, /一个启动器。官方 Codex，以及跑在你自己服务器上的 Claude。/);
  assert.doesNotMatch(signedOut, /更快开始使用官方 Codex。/);
  assert.doesNotMatch(
    signedOut,
    /这个小工具只做一件事：帮你完成注册、登录和本机配置，省去手动安装配置的步骤。之后你使用的始终是官方 Codex 应用，一切保持原生。/,
  );
});

test("the credential promise describes the storage this release actually ships", async () => {
  const signedOut = await readFile(new URL("./views/SignedOutView.tsx", import.meta.url), "utf8");
  const home = await readFile(new URL("./views/HomeView.tsx", import.meta.url), "utf8");

  // 本期凭据落本机受限权限文件（ADR-0001），承诺卡不得再声称钥匙串 / 凭据管理器。
  for (const source of [signedOut, home]) {
    assert.doesNotMatch(source, /钥匙串/);
    assert.doesNotMatch(source, /凭据管理器/);
  }
  assert.match(signedOut, /只保存在这台电脑上/);
  assert.match(signedOut, /界面与日志不含明文/);
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

test("the register form has somewhere to type the invitation code it demands", async () => {
  const view = await readFile(new URL("./views/RegisterView.tsx", import.meta.url), "utf8");

  assert.match(view, /邀请码/);
  assert.match(view, /invitationCode/);
  // 显示与否由服务端开关决定；开关缺席时靠错误码兜底展开并聚焦，见交互规格 §7。
  assert.match(view, /invitationCodeEnabled/);
  assert.match(view, /AUTH_INVITATION_CODE_REQUIRED/);
  assert.match(view, /AUTH_INVITATION_CODE_INVALID/);
  assert.match(view, /focus\(\)/);
});

test("neither auth view hardcodes business rules the server owns", async () => {
  const register = await readFile(new URL("./views/RegisterView.tsx", import.meta.url), "utf8");

  assert.match(register, /settings\.emailSuffixWhitelist|formatEmailSuffixHint/);
  assert.match(register, /settings\.agreementDocuments/);
  assert.doesNotMatch(register, /@gmail\.com|@qq\.com/);
});

test("the provisioning view carries the spec copy for slow and failed steps", async () => {
  const view = await readFile(new URL("./views/ProvisioningView.tsx", import.meta.url), "utf8");

  assert.match(view, /<h1>正在准备<\/h1>/);
  assert.doesNotMatch(view, /正在准备官方 Codex/);
  assert.match(view, /不需要手动操作，完成后自动进入首页/);
  assert.match(view, /比平时慢一些，仍在继续…/);
  assert.match(view, /重试/);
  assert.match(view, /稍后处理/);
  assert.match(view, /PROVISIONING_STEP_TITLES/);
  assert.doesNotMatch(view, /返回/, "the provisioning page must not offer a back exit");
});

test("provisioning feeds the verified account back into the state machine", async () => {
  const view = await readFile(new URL("./views/ProvisioningView.tsx", import.meta.url), "utf8");
  const shell = await readFile(new URL("../LumioApp.tsx", import.meta.url), "utf8");
  const invoke = await readFile(new URL("./invoke.ts", import.meta.url), "utf8");

  // `verify-account` 拉到的是真实 profile；不接住它，首页就会把 bootstrap 的占位余额当真值渲染。
  assert.match(view, /result\.account/);
  assert.match(view, /onAccountResolved/);
  assert.match(shell, /type: "account-refreshed"/);
  // 漏字段时 `undefined !== null` 为真，会把假账户推进首页黑屏；边界必须先归一化。
  assert.match(invoke, /normalizeOptionalAccount/);
  assert.match(view, /if \(result\.account\)/);
  assert.doesNotMatch(view, /result\.account !== null/);
  assert.match(shell, /if \(!current\.account\)/);
});

test("the home surface can install the official app instead of sending users to settings", async () => {
  const home = await readFile(new URL("./views/HomeView.tsx", import.meta.url), "utf8");

  assert.match(home, /安装并启动官方 Codex/);
  assert.match(home, /正在安装官方 Codex/);
  assert.match(home, /安装官方应用需要网络/);
  assert.match(home, /连上之后再回来装。/);
  assert.doesNotMatch(home, /侧载|MSIX|FE3|Sparkle|镜像站/i);
});

test("failed or cancelled official-app install relabels the primary button as 重试", async () => {
  const home = await readFile(new URL("./views/HomeView.tsx", import.meta.url), "utf8");

  assert.match(home, /"重试"/);
  assert.match(home, /installEnded/);
  assert.match(home, /officialAppInstall\.phase === "failed"/);
  assert.match(home, /officialAppInstall\.phase === "cancelled"/);
});

test("in-progress official-app install wires cancelOfficialApp", async () => {
  const home = await readFile(new URL("./views/HomeView.tsx", import.meta.url), "utf8");

  assert.match(home, /cancelOfficialApp/);
  assert.match(home, /installInProgress/);
});

test("the install progress line receives the chosen destination", async () => {
  const home = await readFile(new URL("./views/HomeView.tsx", import.meta.url), "utf8");

  assert.match(home, /resolveInstallDestination/);
  assert.match(home, /installProgressCopy\(/);
});

test("the failure card and repair page open the BestCodex help URL", async () => {
  const home = await readFile(new URL("./views/HomeView.tsx", import.meta.url), "utf8");
  const repair = await readFile(new URL("./views/RepairView.tsx", import.meta.url), "utf8");

  assert.match(home, /打开帮助/);
  assert.match(home, /HELP_URL/);
  assert.match(repair, /打开帮助/);
  assert.match(repair, /HELP_URL/);
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
  assert.match(view, /codexApp === null\s*\?[\s\S]{0,80}安装官方应用需要网络/);
  assert.match(view, /已重新连接/);
  assert.match(view, /缓存值/);
  // 离线首页可能在没有任何可信同步时间时进入，这时余额与时间都不许当作真值渲染。
  assert.match(view, /上次同步时间未知/);
  assert.match(view, /尚未同步/);
});

test("the Codex tab is a greeting, a balance line, and one launch card", async () => {
  const home = await readFile(new URL("./views/HomeView.tsx", import.meta.url), "utf8");

  assert.doesNotMatch(home, /lumio-metric-grid/);
  assert.doesNotMatch(home, /余额与套餐/);
  assert.match(home, /你好，/);
  assert.match(home, /greetingNameFromEmail/);
  assert.match(home, /lumio-launch-card/);
  assert.match(home, /Codex 已就绪/);
  assert.match(home, /可启动/);
  assert.match(home, /尚未安装官方 Codex/);
  assert.match(home, /离线可用/);
  assert.match(home, /需要网络/);
});

test("the shell footer names BestCodex and the version, not Lumio or an internal-channel brand line", async () => {
  const shell = await readFile(new URL("../LumioApp.tsx", import.meta.url), "utf8");

  assert.match(shell, /lumio-footer/);
  assert.match(shell, /<span>BestCodex<\/span>/);
  assert.match(shell, /lumio-footer-version/);
  assert.match(shell, /state\.bootstrap\.version/);
  assert.doesNotMatch(shell, /内部测试渠道/);
  assert.doesNotMatch(shell, />Lumio</);
});

test("the shell chrome uses Codex and Claude tabs and keeps HomeView mounted", async () => {
  const shell = await readFile(new URL("../LumioApp.tsx", import.meta.url), "utf8");

  assert.match(shell, /BestCodex/);
  assert.match(shell, /shellLabels\.codex/);
  assert.match(shell, /shellLabels\.claude/);
  assert.doesNotMatch(shell, /shellLabels\.home/);
  assert.match(shell, /import \{ ClaudeWorkspace \}/);
  assert.match(shell, /<HomeView/);
  assert.match(shell, /<ClaudeWorkspace/);
  assert.doesNotMatch(shell, /\{showClaude \? \(/);
  assert.match(shell, /hidden=\{!showClaude\}/);
  assert.match(shell, /hidden=/);
  assert.match(shell, /aria-hidden=/);
  assert.match(shell, /HELP_URL/);
  assert.match(shell, /openInBrowser\(HELP_URL\)/);
  assert.match(shell, /provisioning: "正在准备"/);
  assert.doesNotMatch(shell, /正在准备连接/);
});

test("the titlebar leaves room for macOS traffic lights and is a drag region", async () => {
  const css = await readFile(new URL("../lumio-shell.css", import.meta.url), "utf8");

  assert.match(css, /prefers-color-scheme:\s*dark/);
  assert.match(css, /-webkit-app-region:\s*drag/);
  assert.match(css, /no-drag/);
  assert.match(css, /78px/);
});

test("settings is five groups rather than a third product tab", async () => {
  const view = await readFile(new URL("./views/SettingsView.tsx", import.meta.url), "utf8");

  assert.match(view, /id="account"|账户/);
  assert.match(view, /准备一台服务器/);
  assert.match(view, /帮助中心/);
  assert.match(view, /通用/);
  assert.match(view, /支持/);
});

test("settings sidebar switches panes instead of jumping to in-page anchors", async () => {
  const view = await readFile(new URL("./views/SettingsView.tsx", import.meta.url), "utf8");

  assert.doesNotMatch(view, /href="#account"/);
  assert.doesNotMatch(view, /href="#codex"/);
  assert.doesNotMatch(view, /href="#claude"/);
  assert.doesNotMatch(view, /href="#general"/);
  assert.doesNotMatch(view, /href="#support"/);
  assert.match(view, /role="tablist"/);
  assert.match(view, /role="tab"/);
  assert.match(view, /role="tabpanel"/);
  assert.match(view, /useState<SettingsSection>\("account"\)/);
});

/**
 * 只断言「离线 / 修复文案存在」证明不了用户走得到它们——上一轮离线缺口正是这样通过审查的。
 * 这里把启动编排整段取出来断言，语义是：这两个阶段的入口与探活 / 健康检查的结果绑在同一处决策里。
 */
function startupPlan(shell: string): string {
  const start = shell.indexOf("async function planStartup");
  assert.notEqual(start, -1, "LumioApp must own an explicit startup orchestration step");
  const end = shell.indexOf("\n}", start);
  assert.notEqual(end, -1, "the startup orchestration must be a closed function body");
  return shell.slice(start, end);
}

test("startup orchestration decides the phase from the probe and the health check", async () => {
  const plan = startupPlan(await readFile(new URL("../LumioApp.tsx", import.meta.url), "utf8"));

  assert.match(plan, /loadLumioBootstrap\(\)/);
  assert.match(plan, /checkTakeover\(\)/);
  assert.match(plan, /"conflicted"/);
  assert.match(plan, /type: "repair-required"/);
  assert.match(plan, /loadPublicSettings\(\)/);
  assert.match(plan, /type: "offline-ready"/);
});

test("the offline entry reports an unknown sync time instead of inventing one", async () => {
  const plan = startupPlan(await readFile(new URL("../LumioApp.tsx", import.meta.url), "utf8"));

  assert.match(plan, /cachedAt: null/);
  assert.doesNotMatch(plan, /new Date\(\)/);
});

test("the consistency check runs before provisioning may touch the local config", async () => {
  const plan = startupPlan(await readFile(new URL("../LumioApp.tsx", import.meta.url), "utf8"));

  assert.ok(
    plan.indexOf("checkTakeover()") < plan.indexOf("loadPublicSettings()"),
    "a conflicted config must be caught before the surface can reach write-config",
  );
});

test("the repair view offers the three spec actions and never a force overwrite", async () => {
  const view = await readFile(new URL("./views/RepairView.tsx", import.meta.url), "utf8");

  assert.match(view, /需要检查配置/);
  assert.match(view, /重新检查/);
  assert.match(view, /恢复本机配置/);
  // restore 是整文件回滚，不是字段级撤销：二次确认必须说清接管后的改动会丢失。
  assert.match(view, /还原到 BestCodex 接管前的状态/);
  assert.match(view, /接管之后你在这个文件里做的修改都会丢失/);
  assert.doesNotMatch(view, /你的其他本机设置不受影响/);
  assert.doesNotMatch(view, /\bLumio\b/);
  assert.match(view, /导出诊断日志/);
  assert.match(view, /导出前会再次扫描并移除敏感内容/);
  assert.match(view, /问题仍未解决/);
  assert.match(view, /本机保存的登录凭据已失效，请重新登录/);
  assert.doesNotMatch(view, /强制覆盖/);
});

test("an expired session takes the same global exit no matter which command hit it", async () => {
  const invoke = await readFile(new URL("./invoke.ts", import.meta.url), "utf8");
  const shell = await readFile(new URL("../LumioApp.tsx", import.meta.url), "utf8");

  // 归一化在命令层：调用点各写一遍就会漏掉一个，漏掉的那个会把用户留在陈旧数据上。
  assert.match(invoke, /onSessionExpired/);
  assert.match(invoke, /reportSessionExpiry/);
  assert.match(shell, /onSessionExpired\(/);
  assert.match(shell, /type: "session-expired"/);
});

test("the settings page offers a way back to the signed-out surface", async () => {
  const view = await readFile(new URL("./views/SettingsView.tsx", import.meta.url), "utf8");

  assert.match(view, /退出登录/);
  assert.match(view, /onSignOut/);
});

test("the settings view keeps every approved row and its explanation", async () => {
  const view = await readFile(new URL("./views/SettingsView.tsx", import.meta.url), "utf8");

  assert.match(view, /你仍会收到重要安全更新提示/);
  assert.match(view, /重新检测/);
  assert.match(view, /手动选择…/);
  assert.match(view, /未检测到，可手动选择/);
  assert.match(view, /所选应用无法识别为官方 Codex/);
  assert.match(view, /不可用的选项会保持禁用，不会修改本机配置。/);
  assert.doesNotMatch(view, /撤销 Lumio 管理的字段并保留其他本机设置/);
  assert.match(view, /再次使用 BestCodex 连接/);
  assert.doesNotMatch(view, /再次使用 Lumio 连接/);
  assert.match(view, /把配置文件还原到接管前的状态/);
  assert.match(view, /使用数据收集尚未开放/);
  assert.match(view, /TELEMETRY_NOTE/);
  assert.doesNotMatch(view, /setTelemetryConfirmOpen\(true\)/);
  assert.doesNotMatch(view, /确认开启/);
});

test("the telemetry switch stays disabled with a stated reason this cycle", async () => {
  const view = await readFile(new URL("./views/SettingsView.tsx", import.meta.url), "utf8");

  assert.match(view, /使用数据收集尚未开放/);
  assert.match(view, /<Toggle checked=\{false\} disabled label=\{shellLabels\.telemetry\} \/>/);
});

test("the home surface opens payment in the browser when online", async () => {
  const view = await readFile(new URL("./views/HomeView.tsx", import.meta.url), "utf8");
  // URL 拼装抽到共享模块：首页与引导失败面必须走同一份推导，护栏钉在源头文件上。
  const paymentHelper = await readFile(new URL("./payment.ts", import.meta.url), "utf8");

  assert.match(view, /openInBrowser/);
  assert.match(view, /paymentUrl/);
  assert.match(paymentHelper, /apiBaseUrl/);
  assert.match(paymentHelper, /paymentPath/);
  assert.match(paymentHelper, /\/purchase/);
  // 充值必须挂 api.lumio.games，禁止拼到营销站 siteBaseUrl。
  assert.doesNotMatch(paymentHelper, /siteBaseUrl/);
  assert.doesNotMatch(paymentHelper, /\/payment/);
  assert.match(view, /已在浏览器中打开支付页面/);
  assert.doesNotMatch(view, /充值功能尚未开放/);
});

test("the shell checks for updates and offers an in-app manual update", async () => {
  const shell = await readFile(new URL("../LumioApp.tsx", import.meta.url), "utf8");
  const view = await readFile(new URL("./views/HomeView.tsx", import.meta.url), "utf8");
  const settings = await readFile(new URL("./views/SettingsView.tsx", import.meta.url), "utf8");
  const css = await readFile(new URL("../lumio-shell.css", import.meta.url), "utf8");

  assert.match(shell, /checkUpdate\(/);
  assert.match(shell, /downloadUpdate\(/);
  // 右下角常驻弹窗：受频率闸门控制（忽略版本 + 每天一次），「稍后」持久忽略该版本。
  assert.match(shell, /lumio-update-pop/);
  assert.match(shell, /立即更新/);
  assert.match(shell, /noticeMuted/);
  assert.match(shell, /updateNoticeShown\(/);
  assert.match(shell, /dismissUpdate\(/);
  // 绿色标记：齿轮绿点 + footer 常驻入口，不受弹窗 dismiss 影响。
  assert.match(shell, /lumio-nav-dot/);
  assert.match(shell, /有新版本/);
  assert.match(css, /\.lumio-update-pop/);
  assert.match(css, /\.lumio-nav-dot/);
  // 设置页是绿点的落点：自动更新行提供立即更新入口。
  assert.match(settings, /立即更新/);
  // 首页不再横幅：提示面收敛为弹窗 + 绿标。
  assert.doesNotMatch(view, /updateReminder/);
  assert.doesNotMatch(view, /onUpdateRequested/);
});

test("React entry renders only LumioApp", async () => {
  const main = await readFile(new URL("../main.tsx", import.meta.url), "utf8");

  assert.match(main, /import \{ LumioApp \} from "\.\/LumioApp"/);
  assert.doesNotMatch(main, /from "\.\/App"/);
});
