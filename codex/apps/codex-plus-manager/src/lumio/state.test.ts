import assert from "node:assert/strict";
import test from "node:test";

import { initialLumioState, reduceLumioState } from "./state.ts";
import {
  PROVISIONING_STEP_IDS,
  PROVISIONING_STEP_TITLES,
} from "./state.ts";
import type { LumioServiceSettings, LumioState } from "./state.ts";
import type { LumioCodexApp } from "./types.ts";

function detectedApp(): LumioCodexApp {
  return { path: "/Applications/Codex.app", version: "1.0.0", source: "automatic" };
}

test("bootstrap without account enters signed-out", () => {
  const next = reduceLumioState(initialLumioState(), {
    type: "bootstrapped",
    payload: {
      version: "1.0.0",
      platform: "macos",
      arch: "aarch64",
      codexApp: null,
      account: null,
      telemetryEnabled: false,
      autoUpdateEnabled: true,
    },
  });

  assert.equal(next.phase, "signed-out");
  assert.equal(next.telemetryEnabled, false);
});

test("offline readiness never enables payment or refresh", () => {
  const next = reduceLumioState(signedOutWithApp(), {
    type: "offline-ready",
    cachedAt: "2026-08-11T00:00:00Z",
  });

  assert.equal(next.phase, "ready-offline");
  assert.equal(next.actions.canLaunch, true);
  assert.equal(next.actions.canRefresh, false);
  assert.equal(next.actions.canPay, false);
});

test("bootstrap with an account enters provisioning with actions disabled", () => {
  const next = reduceLumioState(initialLumioState(), {
    type: "bootstrapped",
    payload: {
      version: "1.0.0",
      platform: "windows",
      arch: "x86_64",
      codexApp: null,
      account: {
        email: "user@example.com",
        balance: 12.5,
        planLabel: "Trial",
      },
      telemetryEnabled: true,
      autoUpdateEnabled: false,
    },
  });

  assert.equal(next.phase, "provisioning");
  assert.equal(next.actions.canLaunch, false);
  assert.equal(next.actions.canRefresh, false);
  assert.equal(next.actions.canPay, false);
  assert.equal(next.telemetryEnabled, true);
  assert.equal(next.autoUpdateEnabled, false);
});

test("repair-required preserves no privileged actions", () => {
  const next = reduceLumioState(initialLumioState(), {
    type: "repair-required",
    errorCode: "CODEX_CONFIG_CONFLICT",
  });

  assert.equal(next.phase, "needs-repair");
  assert.equal(next.errorCode, "CODEX_CONFIG_CONFLICT");
  assert.deepEqual(next.actions, {
    canLaunch: false,
    canRefresh: false,
    canPay: false,
    canRegister: false,
    canSignIn: false,
  });
});

// 启动编排判冲突进入修复页后，服务探活轮询仍会派发 settings/不可达事件（QA D-12）：
// 服务可用性只归 serviceAvailable，不许顺手洗掉修复页正在向用户解释的错误码。
function needsRepair(): LumioState {
  return reduceLumioState(initialLumioState(), {
    type: "repair-required",
    errorCode: "CODEX_CONFIG_CONFLICT",
  });
}

test("service recovery does not clear the repair error code", () => {
  const next = reduceLumioState(needsRepair(), {
    type: "service-settings-loaded",
    settings: SERVICE,
  });

  assert.equal(next.phase, "needs-repair");
  assert.equal(next.errorCode, "CODEX_CONFIG_CONFLICT");
  assert.equal(next.serviceAvailable, true);
});

test("service loss does not overwrite the repair error code", () => {
  const next = reduceLumioState(needsRepair(), {
    type: "service-unavailable",
    errorCode: "SERVICE_UNAVAILABLE",
  });

  assert.equal(next.phase, "needs-repair");
  assert.equal(next.errorCode, "CODEX_CONFIG_CONFLICT");
});

const SERVICE: LumioServiceSettings = {
  registrationEnabled: true,
  emailVerifyEnabled: true,
  emailSuffixWhitelist: ["@example.com"],
  passwordResetEnabled: true,
  agreementEnabled: true,
  agreementRevision: "v2026-03",
  agreementDocuments: [{ id: "terms", title: "服务条款", contentMd: "# 条款" }],
  defaultModel: "gpt-example",
  siteBaseUrl: "https://lumio.games",
  paymentPath: "/purchase",
  apiBaseUrl: "https://api.lumio.games",
};

function signedOut(): LumioState {
  return reduceLumioState(initialLumioState(), {
    type: "bootstrapped",
    payload: {
      version: "1.0.0",
      platform: "macos",
      arch: "aarch64",
      codexApp: null,
      account: null,
      telemetryEnabled: false,
      autoUpdateEnabled: true,
    },
  });
}

function signedOutWithApp(): LumioState {
  return reduceLumioState(initialLumioState(), {
    type: "bootstrapped",
    payload: {
      version: "1.0.0",
      platform: "macos",
      arch: "aarch64",
      codexApp: detectedApp(),
      account: null,
      telemetryEnabled: false,
      autoUpdateEnabled: true,
    },
  });
}

function bootedWithAccount(): LumioState {
  return reduceLumioState(initialLumioState(), {
    type: "bootstrapped",
    payload: {
      version: "1.0.0",
      platform: "macos",
      arch: "aarch64",
      codexApp: detectedApp(),
      account: { email: "previous@example.com", balance: 42, planLabel: "Pro" },
      telemetryEnabled: false,
      autoUpdateEnabled: true,
    },
  });
}

function readyOnlineSession(registrationEnabled: boolean): LumioState {
  const withService = reduceLumioState(bootedWithAccount(), {
    type: "service-settings-loaded",
    settings: { ...SERVICE, registrationEnabled },
  });
  return reduceLumioState(withService, {
    type: "online-ready",
    account: { email: "previous@example.com", balance: 42, planLabel: "Pro" },
    cachedAt: "2026-08-12T00:00:00Z",
    defaultModel: "gpt-example",
    codexApp: detectedApp(),
  });
}

test("provisioning step order matches the interaction spec", () => {
  assert.deepEqual(PROVISIONING_STEP_IDS, [
    "verify-account",
    "prepare-connection",
    "sync-models",
    "write-config",
  ]);
  assert.deepEqual(
    PROVISIONING_STEP_IDS.map((id) => PROVISIONING_STEP_TITLES[id]),
    ["验证账户", "准备连接", "同步模型目录", "写入本机配置"],
  );
});

test("service settings load enables both entry points", () => {
  const next = reduceLumioState(signedOut(), { type: "service-settings-loaded", settings: SERVICE });

  assert.equal(next.serviceAvailable, true);
  assert.equal(next.service?.agreementRevision, "v2026-03");
  assert.equal(next.actions.canSignIn, true);
  assert.equal(next.actions.canRegister, true);
  assert.equal(next.errorCode, null);
});

test("registration disabled by the server disables only the register entry", () => {
  const next = reduceLumioState(signedOut(), {
    type: "service-settings-loaded",
    settings: { ...SERVICE, registrationEnabled: false },
  });

  assert.equal(next.actions.canSignIn, true);
  assert.equal(next.actions.canRegister, false);
  assert.equal(next.actionNotes.register, "注册暂未开放");
});

test("service unavailable disables both entry points and explains why", () => {
  const next = reduceLumioState(signedOut(), {
    type: "service-unavailable",
    errorCode: "SERVICE_UNAVAILABLE",
  });

  assert.equal(next.serviceAvailable, false);
  assert.equal(next.actions.canSignIn, false);
  assert.equal(next.actions.canRegister, false);
  assert.equal(next.errorCode, "SERVICE_UNAVAILABLE");
  assert.equal(next.actionNotes.signIn, "服务暂时不可用，稍后自动重试");
});

test("two-factor requirement keeps the user inside the login card", () => {
  const login = reduceLumioState(signedOut(), { type: "auth-step-changed", step: "login" });
  const next = reduceLumioState(login, { type: "two-factor-required" });

  assert.equal(next.phase, "authenticating");
  assert.equal(next.authStep, "two-factor");
});

test("authentication resets provisioning to a clean pending run", () => {
  const running = reduceLumioState(signedOut(), {
    type: "provisioning-step-started",
    step: "verify-account",
  });
  const dirty = reduceLumioState(running, {
    type: "provisioning-step-failed",
    step: "verify-account",
    errorCode: "KEY_PROVISION_FAILED",
  });
  assert.equal(dirty.provisioning.attempts, 1);
  assert.equal(dirty.provisioning.steps["verify-account"], "failed");
  assert.equal(dirty.provisioning.failedStep, "verify-account");

  const next = reduceLumioState(dirty, {
    type: "authenticated",
    account: { email: "user@example.com", balance: 0, planLabel: null },
  });

  assert.equal(next.phase, "provisioning");
  assert.equal(next.authStep, "idle");
  assert.equal(next.provisioning.failedStep, null);
  assert.equal(next.provisioning.errorCode, null);
  assert.equal(next.provisioning.attempts, 0);
  for (const id of PROVISIONING_STEP_IDS) {
    assert.equal(next.provisioning.steps[id], "pending");
  }
});

test("provisioning steps advance independently and record failures", () => {
  const authed = reduceLumioState(signedOut(), {
    type: "authenticated",
    account: { email: "user@example.com", balance: 0, planLabel: null },
  });
  const running = reduceLumioState(authed, {
    type: "provisioning-step-started",
    step: "verify-account",
  });
  assert.equal(running.provisioning.steps["verify-account"], "running");

  const done = reduceLumioState(running, {
    type: "provisioning-step-completed",
    step: "verify-account",
  });
  assert.equal(done.provisioning.steps["verify-account"], "done");

  const failed = reduceLumioState(done, {
    type: "provisioning-step-failed",
    step: "prepare-connection",
    errorCode: "KEY_PROVISION_FAILED",
  });
  assert.equal(failed.phase, "provisioning");
  assert.equal(failed.provisioning.steps["prepare-connection"], "failed");
  assert.equal(failed.provisioning.failedStep, "prepare-connection");
  assert.equal(failed.provisioning.errorCode, "KEY_PROVISION_FAILED");
  assert.equal(failed.provisioning.attempts, 1);
  assert.equal(failed.provisioning.suggestRepair, false);
  assert.equal(failed.provisioning.steps["sync-models"], "pending");
});

test("a second failure on the same run suggests the repair page", () => {
  let state = reduceLumioState(signedOut(), {
    type: "authenticated",
    account: { email: "user@example.com", balance: 0, planLabel: null },
  });
  for (let attempt = 0; attempt < 2; attempt += 1) {
    state = reduceLumioState(state, {
      type: "provisioning-step-failed",
      step: "prepare-connection",
      errorCode: "KEY_PROVISION_FAILED",
    });
  }

  assert.equal(state.provisioning.attempts, 2);
  assert.equal(state.provisioning.suggestRepair, true);
});

test("an empty model catalog failure stays retryable and repair-suggesting", () => {
  let state = reduceLumioState(signedOut(), {
    type: "authenticated",
    account: { email: "user@example.com", balance: 12.5, planLabel: null },
  });
  for (let attempt = 0; attempt < 2; attempt += 1) {
    state = reduceLumioState(state, {
      type: "provisioning-step-failed",
      step: "sync-models",
      errorCode: "SERVICE_MODEL_CATALOG_EMPTY",
    });
  }

  // 空目录与余额不足不同：这是服务端态，重试与修复页引导维持原行为。
  assert.equal(state.provisioning.suggestRepair, true);
  assert.equal(state.actions.canPay, false);
});

test("insufficient balance never suggests repair and stays payable", () => {
  let state = reduceLumioState(signedOut(), {
    type: "authenticated",
    account: { email: "user@example.com", balance: 0, planLabel: null },
  });
  for (let attempt = 0; attempt < 2; attempt += 1) {
    state = reduceLumioState(state, {
      type: "provisioning-step-failed",
      step: "sync-models",
      errorCode: "ACCOUNT_INSUFFICIENT_BALANCE",
    });
  }

  // 修本机配置对余额不足毫无帮助；充值入口必须留在失败面上。
  assert.equal(state.provisioning.attempts, 2);
  assert.equal(state.provisioning.suggestRepair, false);
  assert.equal(state.actions.canPay, true);
});

test("resuming a step withdraws the payment affordance", () => {
  const authed = reduceLumioState(signedOut(), {
    type: "authenticated",
    account: { email: "user@example.com", balance: 0, planLabel: null },
  });
  const failed = reduceLumioState(authed, {
    type: "provisioning-step-failed",
    step: "sync-models",
    errorCode: "ACCOUNT_INSUFFICIENT_BALANCE",
  });
  assert.equal(failed.actions.canPay, true);

  const resumed = reduceLumioState(failed, {
    type: "provisioning-step-started",
    step: "sync-models",
  });
  assert.equal(resumed.actions.canPay, false);

  // 重试后换成别的失败码，充值入口不得复活。
  const other = reduceLumioState(resumed, {
    type: "provisioning-step-failed",
    step: "sync-models",
    errorCode: "SERVICE_MODEL_CATALOG_EMPTY",
  });
  assert.equal(other.actions.canPay, false);
});

test("online readiness enables launch, refresh, and payment", () => {
  const next = reduceLumioState(signedOut(), {
    type: "online-ready",
    account: { email: "user@example.com", balance: 12.5, planLabel: "Trial" },
    cachedAt: "2026-08-12T00:00:00Z",
    defaultModel: "gpt-example",
    codexApp: { path: "/Applications/Codex.app", version: "1.0.0", source: "automatic" },
  });

  assert.equal(next.phase, "ready-online");
  assert.equal(next.actions.canLaunch, true);
  assert.equal(next.actions.canRefresh, true);
  assert.equal(next.actions.canPay, true);
  assert.equal(next.actionNotes.pay, null);
  assert.equal(next.defaultModel, "gpt-example");
});

test("online readiness without a detected app disables launch and explains why", () => {
  const next = reduceLumioState(signedOut(), {
    type: "online-ready",
    account: { email: "user@example.com", balance: 1, planLabel: null },
    cachedAt: "2026-08-12T00:00:00Z",
    defaultModel: "gpt-example",
    codexApp: null,
  });

  assert.equal(next.actions.canLaunch, false);
  assert.equal(next.actionNotes.launch, "未检测到官方应用，去设置中选择");
});

test("offline readiness keeps launch but blocks refresh and payment", () => {
  const next = reduceLumioState(signedOutWithApp(), {
    type: "offline-ready",
    cachedAt: "2026-08-12T00:00:00Z",
  });

  assert.equal(next.phase, "ready-offline");
  assert.equal(next.actions.canLaunch, true);
  assert.equal(next.actions.canRefresh, false);
  assert.equal(next.actions.canPay, false);
  assert.equal(next.actionNotes.launch, null);
  assert.equal(next.actionNotes.refresh, "需要恢复网络连接");
});

test("offline readiness without a detected app disables launch and explains why", () => {
  const next = reduceLumioState(signedOut(), {
    type: "offline-ready",
    cachedAt: "2026-08-12T00:00:00Z",
  });

  assert.equal(next.phase, "ready-offline");
  assert.equal(next.actions.canLaunch, false);
  assert.equal(next.actionNotes.launch, "未检测到官方应用，去设置中选择");
  assert.equal(next.actionNotes.refresh, "需要恢复网络连接");
});

test("offline readiness entered at startup carries no invented sync time", () => {
  const next = reduceLumioState(signedOutWithApp(), { type: "offline-ready", cachedAt: null });

  assert.equal(next.phase, "ready-offline");
  assert.equal(next.cachedAt, null);
  assert.equal(next.actions.canLaunch, true);
  assert.equal(next.actions.canRefresh, false);
  assert.equal(next.actionNotes.refresh, "需要恢复网络连接");
});

test("reconnecting from offline restores the online surface", () => {
  const offline = reduceLumioState(signedOut(), {
    type: "offline-ready",
    cachedAt: "2026-08-12T00:00:00Z",
  });
  const next = reduceLumioState(offline, {
    type: "online-ready",
    account: { email: "user@example.com", balance: 3, planLabel: null },
    cachedAt: "2026-08-12T01:00:00Z",
    defaultModel: "gpt-example",
    codexApp: { path: "/Applications/Codex.app", version: null, source: "automatic" },
  });

  assert.equal(next.phase, "ready-online");
  assert.equal(next.cachedAt, "2026-08-12T01:00:00Z");
  assert.equal(next.actions.canRefresh, true);
});

test("signing out clears the account and returns to the signed-out surface", () => {
  const online = reduceLumioState(signedOut(), {
    type: "online-ready",
    account: { email: "user@example.com", balance: 3, planLabel: null },
    cachedAt: "2026-08-12T00:00:00Z",
    defaultModel: "gpt-example",
    codexApp: null,
  });
  const next = reduceLumioState(online, { type: "signed-out" });

  assert.equal(next.phase, "signed-out");
  assert.equal(next.account, null);
  assert.equal(next.authStep, "idle");
  assert.deepEqual(next.actions, {
    canLaunch: false,
    canRefresh: false,
    canPay: false,
    canRegister: false,
    canSignIn: false,
  });
  assert.equal(next.serviceAvailable, false);
  assert.equal(next.actionNotes.signIn, "服务暂时不可用，稍后自动重试");
  assert.equal(next.actionNotes.register, "服务暂时不可用，稍后自动重试");
});

test("signing out without ever loading settings reports the service as unavailable", () => {
  const online = reduceLumioState(signedOut(), {
    type: "online-ready",
    account: { email: "user@example.com", balance: 3, planLabel: null },
    cachedAt: "2026-08-12T00:00:00Z",
    defaultModel: "gpt-example",
    codexApp: null,
  });
  const next = reduceLumioState(online, { type: "signed-out" });

  assert.equal(next.service, null);
  assert.equal(next.serviceAvailable, false);
  assert.equal(next.actionNotes.signIn, "服务暂时不可用，稍后自动重试");
  assert.equal(next.actionNotes.register, "服务暂时不可用，稍后自动重试");
});

test("signing out after the settings loaded leaves both entry points usable", () => {
  const withService = reduceLumioState(signedOut(), {
    type: "service-settings-loaded",
    settings: { ...SERVICE, registrationEnabled: true },
  });
  const online = reduceLumioState(withService, {
    type: "online-ready",
    account: { email: "user@example.com", balance: 3, planLabel: null },
    cachedAt: "2026-08-12T00:00:00Z",
    defaultModel: "gpt-example",
    codexApp: null,
  });
  const next = reduceLumioState(online, { type: "signed-out" });

  assert.equal(next.serviceAvailable, true);
  assert.equal(next.actions.canSignIn, true);
  assert.equal(next.actions.canRegister, true);
  assert.equal(next.actionNotes.signIn, null);
  assert.equal(next.actionNotes.register, null);
});

test("signing out drops the account cached on the bootstrap payload", () => {
  const next = reduceLumioState(readyOnlineSession(true), { type: "signed-out" });

  assert.equal(next.account, null);
  assert.equal(next.bootstrap?.account, null);
  assert.equal(next.bootstrap?.version, "1.0.0");
  assert.deepEqual(next.codexApp, detectedApp());
});

test("signing out while the service is reachable keeps both entry points open", () => {
  const next = reduceLumioState(readyOnlineSession(true), { type: "signed-out" });

  assert.equal(next.serviceAvailable, true);
  assert.equal(next.actions.canSignIn, true);
  assert.equal(next.actions.canRegister, true);
  assert.equal(next.actionNotes.signIn, null);
  assert.equal(next.actionNotes.register, null);
  assert.equal(next.actionNotes.pay, null);
});

test("signing out with registration closed explains the disabled register entry", () => {
  const next = reduceLumioState(readyOnlineSession(false), { type: "signed-out" });

  assert.equal(next.actions.canSignIn, true);
  assert.equal(next.actions.canRegister, false);
  assert.equal(next.actionNotes.signIn, null);
  assert.equal(next.actionNotes.register, "注册暂未开放");
});

test("session expiry while the service is unreachable explains both entry points", () => {
  const unreachable = reduceLumioState(readyOnlineSession(true), {
    type: "service-unavailable",
    errorCode: "SERVICE_UNAVAILABLE",
  });
  const next = reduceLumioState(unreachable, {
    type: "session-expired",
    errorCode: "AUTH_SESSION_EXPIRED",
  });

  assert.equal(next.serviceAvailable, false);
  assert.equal(next.actions.canSignIn, false);
  assert.equal(next.actions.canRegister, false);
  assert.equal(next.actionNotes.signIn, "服务暂时不可用，稍后自动重试");
  assert.equal(next.actionNotes.register, "服务暂时不可用，稍后自动重试");
  assert.equal(next.errorCode, "AUTH_SESSION_EXPIRED");
});

test("session expiry while the service is reachable keeps the login entry open", () => {
  const next = reduceLumioState(readyOnlineSession(true), {
    type: "session-expired",
    errorCode: "AUTH_SESSION_EXPIRED",
  });

  assert.equal(next.actions.canSignIn, true);
  assert.equal(next.actionNotes.signIn, null);
  assert.equal(next.errorCode, "AUTH_SESSION_EXPIRED");
});

test("session expiry from any phase lands on signed-out with the code preserved", () => {
  const dirty = reduceLumioState(readyOnlineSession(true), {
    type: "provisioning-step-failed",
    step: "sync-models",
    errorCode: "KEY_PROVISION_FAILED",
  });
  const next = reduceLumioState(dirty, {
    type: "session-expired",
    errorCode: "AUTH_SESSION_EXPIRED",
  });

  assert.equal(next.phase, "signed-out");
  assert.equal(next.account, null);
  assert.equal(next.errorCode, "AUTH_SESSION_EXPIRED");
  assert.equal(next.bootstrap?.account, null);
  assert.deepEqual(next.codexApp, detectedApp());
  assert.equal(next.provisioning.failedStep, null);
  assert.equal(next.provisioning.errorCode, null);
  assert.equal(next.provisioning.attempts, 0);
  assert.equal(next.provisioning.suggestRepair, false);
  for (const id of PROVISIONING_STEP_IDS) {
    assert.equal(next.provisioning.steps[id], "pending");
  }
});

test("a late provisioning failure cannot yank the user back from the login entry", () => {
  // D-2：runCommand 在会话过期时先同步派发 session-expired（随后监听器派发
  // auth-step-changed: login）再 rethrow，ProvisioningView 的 catch 稍后才派发
  // provisioning-step-failed——迟到的失败不得覆盖已经就位的登录入口。
  const expired = reduceLumioState(readyOnlineSession(true), {
    type: "session-expired",
    errorCode: "AUTH_SESSION_EXPIRED",
  });
  const atLogin = reduceLumioState(expired, { type: "auth-step-changed", step: "login" });
  const next = reduceLumioState(atLogin, {
    type: "provisioning-step-failed",
    step: "verify-account",
    errorCode: "AUTH_SESSION_EXPIRED",
  });

  assert.equal(next.phase, "authenticating");
  assert.equal(next.provisioning.failedStep, null);
  assert.equal(next.provisioning.errorCode, null);
});

test("a late provisioning failure also leaves plain signed-out alone", () => {
  const expired = reduceLumioState(readyOnlineSession(true), {
    type: "session-expired",
    errorCode: "AUTH_SESSION_EXPIRED",
  });
  const next = reduceLumioState(expired, {
    type: "provisioning-step-failed",
    step: "verify-account",
    errorCode: "AUTH_SESSION_EXPIRED",
  });

  assert.equal(next.phase, "signed-out");
});

test("manually picking the app while offline re-enables the launch button", () => {
  // D-3：离线首页用 state.codexApp 判定 canLaunch；ready-offline 下手选应用不得被丢弃。
  const offline = reduceLumioState(signedOut(), {
    type: "offline-ready",
    cachedAt: "2026-08-12T00:00:00Z",
  });
  assert.equal(offline.actions.canLaunch, false);
  assert.equal(offline.actionNotes.launch, "未检测到官方应用，去设置中选择");

  const next = reduceLumioState(offline, { type: "codex-app-changed", app: detectedApp() });

  assert.equal(next.phase, "ready-offline");
  assert.equal(next.actions.canLaunch, true);
  assert.equal(next.actionNotes.launch, null);
  assert.deepEqual(next.codexApp, detectedApp());
});

test("manually picking the app while signed out is remembered for the offline home", () => {
  const picked = reduceLumioState(signedOut(), { type: "codex-app-changed", app: detectedApp() });

  // 登出态本身不能启动，但选择被保留，进入离线首页时立即生效。
  assert.equal(picked.phase, "signed-out");
  assert.equal(picked.actions.canLaunch, false);
  assert.deepEqual(picked.codexApp, detectedApp());

  const offline = reduceLumioState(picked, { type: "offline-ready", cachedAt: null });
  assert.equal(offline.actions.canLaunch, true);
  assert.equal(offline.actionNotes.launch, null);
});

test("manually picking the app while online keeps the ready home consistent", () => {
  const next = reduceLumioState(readyOnlineSession(true), {
    type: "codex-app-changed",
    app: detectedApp(),
  });

  assert.equal(next.phase, "ready-online");
  assert.equal(next.actions.canLaunch, true);
  assert.equal(next.actionNotes.launch, null);
});

test("account refresh updates the balance without changing phase", () => {
  const online = reduceLumioState(signedOut(), {
    type: "online-ready",
    account: { email: "user@example.com", balance: 3, planLabel: null },
    cachedAt: "2026-08-12T00:00:00Z",
    defaultModel: "gpt-example",
    codexApp: null,
  });
  const next = reduceLumioState(online, {
    type: "account-refreshed",
    account: { email: "user@example.com", balance: 9.75, planLabel: "Pro" },
    cachedAt: "2026-08-12T02:00:00Z",
  });

  assert.equal(next.phase, "ready-online");
  assert.equal(next.account?.balance, 9.75);
  assert.equal(next.cachedAt, "2026-08-12T02:00:00Z");
});

test("reducer never mutates the state it was given", () => {
  const before = signedOut();
  const snapshot = JSON.stringify(before);
  reduceLumioState(before, { type: "service-settings-loaded", settings: SERVICE });

  assert.equal(JSON.stringify(before), snapshot);
});
