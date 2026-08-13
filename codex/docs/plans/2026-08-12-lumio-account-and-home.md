# Lumio Codex 桌面端账户与首页交互 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use subagent-driven-development to implement this plan task-by-task (hosts without subagents: its Inline Fallback section). Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在现有 Lumio 外壳之上实现注册、登录（含 2FA）、自动配置、首页（在线 / 离线）、needs-repair、设置六块交互，对接 Sub2API 既有认证接口，并完成官方 Codex 配置接管与启动。

**Architecture:** 业务逻辑全部下沉到 `crates/codex-plus-core/src/lumio/`（该 crate 已有 `reqwest` / `sha2` / `toml_edit` / `fs2` / `uuid`，无需改任何 `Cargo.toml`）；`apps/codex-plus-manager/src-tauri/src/lumio_commands.rs` 只做薄命令层，负责 IPC 边界的脱敏；React 侧把纯逻辑（状态机、错误文案映射、表单校验）抽成可被 `node --test` 覆盖的 `.ts` 模块，视图组件只消费这些纯函数。

**Tech Stack:** Rust 2024 / Tauri 2 / reqwest 0.12 / toml_edit 0.22 / React 19 / TypeScript 5.8 / Node 22 原生 `--test` + type stripping

## Global Constraints

以下每条约束对**每个**任务都生效，实现者必须逐条遵守。

- **禁改依赖清单**：不得修改 `Cargo.toml`、`package.json`、`.gitignore`。所有 Rust 新代码放在 `crates/codex-plus-core`（已具备全部所需依赖）或 `apps/codex-plus-manager/src-tauri`（只用其现有依赖 `anyhow` / `serde` / `serde_json` / `tauri` / `codex-plus-core`）。前端不得引入新 npm 包。
- **命令白名单只增 `lumio_` 前缀命令**：`apps/codex-plus-manager/src-tauri/src/lib.rs` 的 `invoke_handler` 内只允许出现 `lumio_commands::lumio_*`、`lumio_hide_to_tray`、`lumio_exit_app`。旧 `commands::*` 一个都不许注册。
- **禁词红线**：UI 可见文案与 `shellLabels` 清单中不得出现 `provider`、`base url`、`api key`、`stepwise`、`mcp`、`plugin`、`dream skin`（大小写不敏感）。`apps/codex-plus-manager/src/lumio/shell-copy.test.ts` 的禁词断言必须始终通过。此红线约束**用户可见文案**，不约束官方 Codex 配置文件本身的 TOML 键名（那是官方 schema，只存在于 Rust core 内部，不上屏）。
- **秘密不跨 IPC**：任何 Tauri 命令的返回值都不得包含访问令牌、刷新令牌、临时 2FA token 或 API Key 明文。前端只能拿到 `"present" | "missing" | "invalid"` 三态。日志与错误对象同样不得含秘密。
- **文案权威**：所有中文文案以 `docs/specs/2026-08-12-lumio-ux-interaction-design.md` 第 5 节与第 7 节为准，一字不差；本计划中给出的字符串即为最终值，直接照抄。
- **视觉克制**：不得新增品牌渐变、极光、轨道动效类装饰。现有 `lumio-aurora*` 与 `lumio-orbit*` 属于要被移除的营销化视觉（交互文档 §1.0 与 §5.1「不放品牌动效」）。主按钮为中性浅色实底，彩色只用于状态语义。
- **可擦除 TS 语法**：前端测试由 Node 22 的 type stripping 直接执行 `.ts`，因此生产代码**不得使用** `enum`、`namespace`、构造函数参数属性等不可擦除语法。用 `as const` 对象 + `typeof` 派生联合类型代替 `enum`。
- **TDD 铁律**：没有先失败的测试就没有生产代码。每个任务的第一步都是写测试并跑到失败。
- **本期范围外**：支付交接、自动更新、遥测的真实发送、官网。涉及处保持禁用态 + 状态说明，不得伪装成功。
- **服务端契约权威**：`/Users/cui/Sites/sub2api`（只读参考）。统一 envelope 为 `{"code":0,"message":"success","data":{...}}`，失败时 `code` 等于 HTTP 状态码且 `reason` 为机器可读常量。业务判断只读 `reason`，绝不匹配 `message` 字符串。
- **分支**：在当前分支逐任务提交，不 push，不开 PR。

---

## 文件结构

**新建 — Rust（`crates/codex-plus-core/src/lumio/`）**

| 文件 | 职责 |
|------|------|
| `errors.rs` | Lumio 稳定错误码枚举 + 服务端 `reason` → 错误码归一化 + 秘密脱敏器 |
| `api.rs` | Sub2API HTTP 契约：envelope 解析、超时、各认证/账户端点 |
| `credentials.rs` | 本地凭据文件读写（原子写 + 0600），对外只暴露三态 |
| `account.rs` | 账户编排：登录/注册/2FA/刷新/登出、桌面 Key 查找或创建 |
| `config_takeover.rs` | 官方 Codex 配置接管：快照、字段级合并、原子写、外部修改检测、恢复 |
| `launch.rs` | 无注入地启动官方 Codex、在系统浏览器打开 URL |

**修改 — Rust**

| 文件 | 改动 |
|------|------|
| `crates/codex-plus-core/src/lumio/mod.rs` | 挂载上述 6 个新模块 |
| `apps/codex-plus-manager/src-tauri/src/lumio_commands.rs` | 新增全部 `lumio_*` 命令 |
| `apps/codex-plus-manager/src-tauri/src/lib.rs` | 扩充 `invoke_handler` 白名单 |
| `apps/codex-plus-manager/src-tauri/tests/lumio_command_surface.rs` | 同步白名单断言 |

**新建 — 前端（`apps/codex-plus-manager/src/lumio/`）**

| 文件 | 职责 |
|------|------|
| `errors.ts` / `errors.test.ts` | 错误码 → 中文文案的单一映射模块 |
| `forms.ts` / `forms.test.ts` | 纯表单校验：邮箱格式、后缀白名单、密码强度、验证码过滤、协议勾选完备性 |
| `views/SignedOutView.tsx` | 5.1 未登录首页 |
| `views/RegisterView.tsx` | 5.2 注册页 |
| `views/LoginView.tsx` | 5.3 登录页（含 2FA 分格输入） |
| `views/ProvisioningView.tsx` | 5.4 四步进度页 |
| `views/HomeView.tsx` | 5.5 首页在线 / 离线 |
| `views/RepairView.tsx` | 5.7 修复页 |
| `views/SettingsView.tsx` | 5.8 设置页 |
| `views/Toast.tsx` | §6 全局 toast 容器 |

**修改 — 前端**

| 文件 | 改动 |
|------|------|
| `src/lumio/types.ts` | 扩充契约类型 |
| `src/lumio/state.ts` / `state.test.ts` | 完整事件流状态机 |
| `src/lumio/invoke.ts` / `shell-copy.test.ts` | 新命令绑定 + 文案清单 |
| `src/LumioApp.tsx` | 收缩为壳层 + 视图路由 |
| `src/lumio-shell.css` | 移除 aurora / orbit，新增各视图样式 |

---

### Task 1: 错误码 → 文案映射模块（前端）

**Files:**
- Create: `apps/codex-plus-manager/src/lumio/errors.ts`
- Test: `apps/codex-plus-manager/src/lumio/errors.test.ts`

**Interfaces:**
- Consumes: 无（本计划的第一个任务）
- Produces: `LUMIO_ERROR_COPY: Record<string, string>`、`lumioErrorCopy(code: string | null | undefined): string`、`lumioErrorLabel(code: string | null | undefined): string`。后续所有视图取错误文案只经这两个函数。

- [ ] **Step 1: Write the failing test**

创建 `apps/codex-plus-manager/src/lumio/errors.test.ts`：

```ts
import assert from "node:assert/strict";
import test from "node:test";

import { LUMIO_ERROR_COPY, lumioErrorCopy, lumioErrorLabel } from "./errors.ts";

test("interaction spec baseline codes map to their exact copy", () => {
  assert.equal(LUMIO_ERROR_COPY.AUTH_INVALID_CREDENTIALS, "邮箱或密码不正确");
  assert.equal(LUMIO_ERROR_COPY.AUTH_CODE_INVALID, "验证码不正确或已过期");
  assert.equal(LUMIO_ERROR_COPY.AUTH_CODE_RATE_LIMITED, "发送太频繁，请稍后再试");
  assert.equal(LUMIO_ERROR_COPY.AUTH_EMAIL_DOMAIN_NOT_ALLOWED, "该邮箱后缀暂不支持");
  assert.equal(LUMIO_ERROR_COPY.AUTH_REGISTRATION_CLOSED, "注册暂未开放");
  assert.equal(LUMIO_ERROR_COPY.AUTH_2FA_INVALID, "两步验证码不正确");
  assert.equal(LUMIO_ERROR_COPY.AUTH_ACCOUNT_DISABLED, "该账户已被停用");
  assert.equal(LUMIO_ERROR_COPY.AUTH_SESSION_EXPIRED, "登录已过期，请重新登录");
  assert.equal(LUMIO_ERROR_COPY.KEY_PROVISION_FAILED, "连接初始化失败，可重试");
  assert.equal(LUMIO_ERROR_COPY.KEY_STORAGE_UNAVAILABLE, "无法访问系统安全存储");
  assert.equal(LUMIO_ERROR_COPY.SERVICE_UNAVAILABLE, "服务暂时不可用，稍后自动重试");
  assert.equal(LUMIO_ERROR_COPY.SERVICE_VERSION_TOO_OLD, "当前版本过旧，请更新后继续");
  assert.equal(LUMIO_ERROR_COPY.CODEX_APP_NOT_FOUND, "未检测到官方应用");
  assert.equal(LUMIO_ERROR_COPY.CODEX_APP_INVALID, "所选应用无法识别为官方 Codex");
  assert.equal(LUMIO_ERROR_COPY.CODEX_CONFIG_CONFLICT, "检测到本机配置被其他工具修改过");
  assert.equal(LUMIO_ERROR_COPY.CODEX_RESTORE_FAILED, "恢复未完成，已保留原始快照");
  assert.equal(LUMIO_ERROR_COPY.CODEX_LAUNCH_FAILED, "启动官方 Codex 失败");
  assert.equal(LUMIO_ERROR_COPY.PAYMENT_HANDOFF_CREATE_FAILED, "暂时无法发起充值");
  assert.equal(LUMIO_ERROR_COPY.PAYMENT_HANDOFF_EXPIRED, "支付链接已过期，请重新打开");
  assert.equal(LUMIO_ERROR_COPY.UPDATE_VERIFY_FAILED, "更新包校验未通过，已放弃安装");
});

test("codes added beyond the baseline stay inside the six approved domains", () => {
  const domains = ["AUTH_", "KEY_", "SERVICE_", "CODEX_", "PAYMENT_HANDOFF_", "UPDATE_"];
  for (const code of Object.keys(LUMIO_ERROR_COPY)) {
    if (code === "UNKNOWN") continue;
    assert.ok(
      domains.some((domain) => code.startsWith(domain)),
      `error code outside the approved domains: ${code}`,
    );
  }
});

test("unknown and empty codes fall back without throwing", () => {
  assert.equal(lumioErrorCopy("NOT_A_REAL_CODE"), "出现未知问题，请稍后重试");
  assert.equal(lumioErrorCopy(null), "出现未知问题，请稍后重试");
  assert.equal(lumioErrorCopy(undefined), "出现未知问题，请稍后重试");
});

test("labels append the code chip so users can quote it to support", () => {
  assert.equal(
    lumioErrorLabel("AUTH_INVALID_CREDENTIALS"),
    "邮箱或密码不正确（AUTH_INVALID_CREDENTIALS）",
  );
  assert.equal(lumioErrorLabel(null), "出现未知问题，请稍后重试（UNKNOWN）");
});

test("copy never leaks forbidden product surfaces", () => {
  const copy = Object.values(LUMIO_ERROR_COPY).join(" ").toLowerCase();
  for (const forbidden of ["provider", "base url", "api key", "stepwise", "mcp", "plugin", "dream skin"]) {
    assert.equal(copy.includes(forbidden), false, `forbidden term in error copy: ${forbidden}`);
  }
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/codex-plus-manager && node --test src/lumio/errors.test.ts`
Expected: FAIL — `Cannot find module './errors.ts'`

- [ ] **Step 3: Write minimal implementation**

创建 `apps/codex-plus-manager/src/lumio/errors.ts`：

```ts
export const LUMIO_ERROR_COPY: Record<string, string> = {
  AUTH_INVALID_CREDENTIALS: "邮箱或密码不正确",
  AUTH_CODE_INVALID: "验证码不正确或已过期",
  AUTH_CODE_REQUIRED: "请先获取邮箱验证码",
  AUTH_CODE_RATE_LIMITED: "发送太频繁，请稍后再试",
  AUTH_EMAIL_DOMAIN_NOT_ALLOWED: "该邮箱后缀暂不支持",
  AUTH_EMAIL_ALREADY_REGISTERED: "该邮箱已注册，请直接登录",
  AUTH_REGISTRATION_CLOSED: "注册暂未开放",
  AUTH_2FA_INVALID: "两步验证码不正确",
  AUTH_2FA_UNAVAILABLE: "两步验证当前不可用，请联系支持",
  AUTH_ACCOUNT_DISABLED: "该账户已被停用",
  AUTH_SESSION_EXPIRED: "登录已过期，请重新登录",
  KEY_PROVISION_FAILED: "连接初始化失败，可重试",
  KEY_STORAGE_UNAVAILABLE: "无法访问系统安全存储",
  SERVICE_UNAVAILABLE: "服务暂时不可用，稍后自动重试",
  SERVICE_RATE_LIMITED: "请求过于频繁，请稍后再试",
  SERVICE_VERSION_TOO_OLD: "当前版本过旧，请更新后继续",
  CODEX_APP_NOT_FOUND: "未检测到官方应用",
  CODEX_APP_INVALID: "所选应用无法识别为官方 Codex",
  CODEX_CONFIG_CONFLICT: "检测到本机配置被其他工具修改过",
  CODEX_CONFIG_WRITE_FAILED: "写入本机配置失败，已保留原始内容",
  CODEX_RESTORE_FAILED: "恢复未完成，已保留原始快照",
  CODEX_LAUNCH_FAILED: "启动官方 Codex 失败",
  PAYMENT_HANDOFF_CREATE_FAILED: "暂时无法发起充值",
  PAYMENT_HANDOFF_EXPIRED: "支付链接已过期，请重新打开",
  UPDATE_VERIFY_FAILED: "更新包校验未通过，已放弃安装",
  UNKNOWN: "出现未知问题，请稍后重试",
};

const UNKNOWN_CODE = "UNKNOWN";

function normalizeCode(code: string | null | undefined): string {
  if (typeof code !== "string") return UNKNOWN_CODE;
  const trimmed = code.trim();
  if (trimmed === "" || !Object.hasOwn(LUMIO_ERROR_COPY, trimmed)) return UNKNOWN_CODE;
  return trimmed;
}

export function lumioErrorCopy(code: string | null | undefined): string {
  return LUMIO_ERROR_COPY[normalizeCode(code)];
}

export function lumioErrorLabel(code: string | null | undefined): string {
  const resolved = normalizeCode(code);
  return `${LUMIO_ERROR_COPY[resolved]}（${resolved}）`;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd apps/codex-plus-manager && node --test src/lumio/errors.test.ts && npm run check`
Expected: 5 tests pass；`tsc --noEmit` 无输出

- [ ] **Step 5: Commit**

```bash
git add apps/codex-plus-manager/src/lumio/errors.ts apps/codex-plus-manager/src/lumio/errors.test.ts
git commit -m "feat(lumio): add stable error code to copy mapping"
```

---

### Task 2: 前端契约类型与完整状态机

**Files:**
- Modify: `apps/codex-plus-manager/src/lumio/types.ts`
- Modify: `apps/codex-plus-manager/src/lumio/state.ts`
- Test: `apps/codex-plus-manager/src/lumio/state.test.ts`（在现有 4 个测试基础上扩充，现有测试的断言不得删改）

**Interfaces:**
- Consumes: Task 1 的错误码字符串（仅作为值，不 import）
- Produces:
  - 类型：`LumioServiceSettings`、`LumioAgreementDocument`、`LumioAuthStep`、`ProvisioningStepId`、`ProvisioningStepStatus`、`LumioProvisioning`、`LumioActions`、`LumioActionNotes`、`LumioState`、`LumioEvent`
  - 函数：`initialLumioState(): LumioState`、`reduceLumioState(state, event): LumioState`
  - 常量：`PROVISIONING_STEP_IDS: readonly ProvisioningStepId[]`、`PROVISIONING_STEP_TITLES: Record<ProvisioningStepId, string>`

- [ ] **Step 1: Write the failing test**

在 `apps/codex-plus-manager/src/lumio/state.test.ts` 末尾追加（保留文件已有的 4 个测试与其 import）：

```ts
import {
  PROVISIONING_STEP_IDS,
  PROVISIONING_STEP_TITLES,
} from "./state.ts";
import type { LumioServiceSettings, LumioState } from "./state.ts";

const SERVICE: LumioServiceSettings = {
  registrationEnabled: true,
  emailVerifyEnabled: true,
  emailSuffixWhitelist: ["@example.com"],
  passwordResetEnabled: true,
  agreementEnabled: true,
  agreementRevision: "v2026-03",
  agreementDocuments: [{ id: "terms", title: "服务条款", contentMd: "# 条款" }],
  defaultModel: "gpt-example",
  siteBaseUrl: "https://api.lumio.games",
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
  const next = reduceLumioState(signedOut(), {
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

test("online readiness enables launch and refresh but never payment", () => {
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
  assert.equal(next.actions.canPay, false);
  assert.equal(next.actionNotes.pay, "充值功能尚未开放");
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
  const next = reduceLumioState(signedOut(), {
    type: "offline-ready",
    cachedAt: "2026-08-12T00:00:00Z",
  });

  assert.equal(next.phase, "ready-offline");
  assert.equal(next.actions.canLaunch, true);
  assert.equal(next.actions.canRefresh, false);
  assert.equal(next.actions.canPay, false);
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
});

test("session expiry from any phase lands on signed-out with the code preserved", () => {
  const online = reduceLumioState(signedOut(), {
    type: "online-ready",
    account: { email: "user@example.com", balance: 3, planLabel: null },
    cachedAt: "2026-08-12T00:00:00Z",
    defaultModel: "gpt-example",
    codexApp: null,
  });
  const next = reduceLumioState(online, {
    type: "session-expired",
    errorCode: "AUTH_SESSION_EXPIRED",
  });

  assert.equal(next.phase, "signed-out");
  assert.equal(next.account, null);
  assert.equal(next.errorCode, "AUTH_SESSION_EXPIRED");
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/codex-plus-manager && node --test src/lumio/state.test.ts`
Expected: FAIL — `PROVISIONING_STEP_IDS` 等导出不存在

- [ ] **Step 3: Write minimal implementation**

先在 `apps/codex-plus-manager/src/lumio/types.ts` 追加（保留现有 `LumioPhase` / `LumioAccountSummary` / `LumioCodexApp` / `LumioBootstrap` 不变）：

```ts
export interface LumioAgreementDocument {
  id: string;
  title: string;
  contentMd: string;
}

export interface LumioServiceSettings {
  registrationEnabled: boolean;
  emailVerifyEnabled: boolean;
  emailSuffixWhitelist: string[];
  passwordResetEnabled: boolean;
  agreementEnabled: boolean;
  agreementRevision: string;
  agreementDocuments: LumioAgreementDocument[];
  defaultModel: string | null;
  siteBaseUrl: string;
}

export type LumioCredentialStatus = "present" | "missing" | "invalid";
```

再重写 `apps/codex-plus-manager/src/lumio/state.ts`：

```ts
import type {
  LumioAccountSummary,
  LumioBootstrap,
  LumioCodexApp,
  LumioPhase,
  LumioServiceSettings,
} from "./types.ts";

export type { LumioServiceSettings } from "./types.ts";

export const PROVISIONING_STEP_IDS = [
  "verify-account",
  "prepare-connection",
  "sync-models",
  "write-config",
] as const;

export type ProvisioningStepId = (typeof PROVISIONING_STEP_IDS)[number];

export const PROVISIONING_STEP_TITLES: Record<ProvisioningStepId, string> = {
  "verify-account": "验证账户",
  "prepare-connection": "准备连接",
  "sync-models": "同步模型目录",
  "write-config": "写入本机配置",
};

export type ProvisioningStepStatus = "pending" | "running" | "done" | "failed";

export type LumioAuthStep = "idle" | "login" | "register" | "two-factor";

export interface LumioProvisioning {
  steps: Record<ProvisioningStepId, ProvisioningStepStatus>;
  failedStep: ProvisioningStepId | null;
  errorCode: string | null;
  attempts: number;
  suggestRepair: boolean;
}

export interface LumioActions {
  canLaunch: boolean;
  canRefresh: boolean;
  canPay: boolean;
  canRegister: boolean;
  canSignIn: boolean;
}

export interface LumioActionNotes {
  launch: string | null;
  refresh: string | null;
  pay: string | null;
  register: string | null;
  signIn: string | null;
}

export interface LumioState {
  phase: LumioPhase;
  bootstrap: LumioBootstrap | null;
  service: LumioServiceSettings | null;
  serviceAvailable: boolean;
  authStep: LumioAuthStep;
  account: LumioAccountSummary | null;
  codexApp: LumioCodexApp | null;
  defaultModel: string | null;
  provisioning: LumioProvisioning;
  telemetryEnabled: boolean;
  autoUpdateEnabled: boolean;
  cachedAt: string | null;
  errorCode: string | null;
  actions: LumioActions;
  actionNotes: LumioActionNotes;
}

export type LumioEvent =
  | { type: "bootstrapped"; payload: LumioBootstrap }
  | { type: "service-settings-loaded"; settings: LumioServiceSettings }
  | { type: "service-unavailable"; errorCode: string }
  | { type: "auth-step-changed"; step: LumioAuthStep }
  | { type: "two-factor-required" }
  | { type: "authenticated"; account: LumioAccountSummary }
  | { type: "provisioning-step-started"; step: ProvisioningStepId }
  | { type: "provisioning-step-completed"; step: ProvisioningStepId }
  | { type: "provisioning-step-failed"; step: ProvisioningStepId; errorCode: string }
  | {
      type: "online-ready";
      account: LumioAccountSummary;
      cachedAt: string;
      defaultModel: string | null;
      codexApp: LumioCodexApp | null;
    }
  | { type: "offline-ready"; cachedAt: string }
  | { type: "account-refreshed"; account: LumioAccountSummary; cachedAt: string }
  | { type: "repair-required"; errorCode: string }
  | { type: "session-expired"; errorCode: string }
  | { type: "signed-out" };

const PAY_DISABLED_NOTE = "充值功能尚未开放";
const OFFLINE_NOTE = "需要恢复网络连接";
const NO_APP_NOTE = "未检测到官方应用，去设置中选择";
const SERVICE_DOWN_NOTE = "服务暂时不可用，稍后自动重试";
const REGISTRATION_CLOSED_NOTE = "注册暂未开放";
const MAX_PROVISIONING_ATTEMPTS = 2;

function disabledActions(): LumioActions {
  return {
    canLaunch: false,
    canRefresh: false,
    canPay: false,
    canRegister: false,
    canSignIn: false,
  };
}

function noNotes(): LumioActionNotes {
  return { launch: null, refresh: null, pay: PAY_DISABLED_NOTE, register: null, signIn: null };
}

function pendingProvisioning(): LumioProvisioning {
  return {
    steps: {
      "verify-account": "pending",
      "prepare-connection": "pending",
      "sync-models": "pending",
      "write-config": "pending",
    },
    failedStep: null,
    errorCode: null,
    attempts: 0,
    suggestRepair: false,
  };
}

export function initialLumioState(): LumioState {
  return {
    phase: "bootstrapping",
    bootstrap: null,
    service: null,
    serviceAvailable: false,
    authStep: "idle",
    account: null,
    codexApp: null,
    defaultModel: null,
    provisioning: pendingProvisioning(),
    telemetryEnabled: false,
    autoUpdateEnabled: true,
    cachedAt: null,
    errorCode: null,
    actions: disabledActions(),
    actionNotes: noNotes(),
  };
}

function withStepStatus(
  provisioning: LumioProvisioning,
  step: ProvisioningStepId,
  status: ProvisioningStepStatus,
): Record<ProvisioningStepId, ProvisioningStepStatus> {
  return { ...provisioning.steps, [step]: status };
}

export function reduceLumioState(state: LumioState, event: LumioEvent): LumioState {
  switch (event.type) {
    case "bootstrapped":
      return {
        ...state,
        phase: event.payload.account === null ? "signed-out" : "provisioning",
        bootstrap: event.payload,
        account: event.payload.account,
        codexApp: event.payload.codexApp,
        telemetryEnabled: event.payload.telemetryEnabled,
        autoUpdateEnabled: event.payload.autoUpdateEnabled,
        actions: disabledActions(),
        actionNotes: noNotes(),
      };

    case "service-settings-loaded":
      return {
        ...state,
        service: event.settings,
        serviceAvailable: true,
        defaultModel: event.settings.defaultModel,
        errorCode: null,
        actions: {
          ...state.actions,
          canSignIn: true,
          canRegister: event.settings.registrationEnabled,
        },
        actionNotes: {
          ...state.actionNotes,
          signIn: null,
          register: event.settings.registrationEnabled ? null : REGISTRATION_CLOSED_NOTE,
        },
      };

    case "service-unavailable":
      return {
        ...state,
        serviceAvailable: false,
        errorCode: event.errorCode,
        actions: { ...state.actions, canSignIn: false, canRegister: false, canRefresh: false },
        actionNotes: {
          ...state.actionNotes,
          signIn: SERVICE_DOWN_NOTE,
          register: SERVICE_DOWN_NOTE,
          refresh: OFFLINE_NOTE,
        },
      };

    case "auth-step-changed":
      return {
        ...state,
        phase: event.step === "idle" ? "signed-out" : "authenticating",
        authStep: event.step,
        errorCode: null,
      };

    case "two-factor-required":
      return { ...state, phase: "authenticating", authStep: "two-factor", errorCode: null };

    case "authenticated":
      return {
        ...state,
        phase: "provisioning",
        authStep: "idle",
        account: event.account,
        errorCode: null,
        provisioning: pendingProvisioning(),
      };

    case "provisioning-step-started":
      return {
        ...state,
        phase: "provisioning",
        provisioning: {
          ...state.provisioning,
          steps: withStepStatus(state.provisioning, event.step, "running"),
          failedStep: null,
          errorCode: null,
        },
      };

    case "provisioning-step-completed":
      return {
        ...state,
        provisioning: {
          ...state.provisioning,
          steps: withStepStatus(state.provisioning, event.step, "done"),
        },
      };

    case "provisioning-step-failed": {
      const attempts = state.provisioning.attempts + 1;
      return {
        ...state,
        phase: "provisioning",
        provisioning: {
          ...state.provisioning,
          steps: withStepStatus(state.provisioning, event.step, "failed"),
          failedStep: event.step,
          errorCode: event.errorCode,
          attempts,
          suggestRepair: attempts >= MAX_PROVISIONING_ATTEMPTS,
        },
      };
    }

    case "online-ready":
      return {
        ...state,
        phase: "ready-online",
        account: event.account,
        codexApp: event.codexApp,
        defaultModel: event.defaultModel,
        cachedAt: event.cachedAt,
        serviceAvailable: true,
        errorCode: null,
        actions: {
          ...state.actions,
          canLaunch: event.codexApp !== null,
          canRefresh: true,
          canPay: false,
        },
        actionNotes: {
          ...state.actionNotes,
          launch: event.codexApp === null ? NO_APP_NOTE : null,
          refresh: null,
          pay: PAY_DISABLED_NOTE,
        },
      };

    case "offline-ready":
      return {
        ...state,
        phase: "ready-offline",
        cachedAt: event.cachedAt,
        serviceAvailable: false,
        actions: { ...state.actions, canLaunch: true, canRefresh: false, canPay: false },
        actionNotes: {
          ...state.actionNotes,
          launch: null,
          refresh: OFFLINE_NOTE,
          pay: OFFLINE_NOTE,
        },
      };

    case "account-refreshed":
      return { ...state, account: event.account, cachedAt: event.cachedAt };

    case "repair-required":
      return {
        ...state,
        phase: "needs-repair",
        errorCode: event.errorCode,
        actions: disabledActions(),
        actionNotes: noNotes(),
      };

    case "session-expired":
      return {
        ...initialLumioState(),
        phase: "signed-out",
        bootstrap: state.bootstrap,
        service: state.service,
        serviceAvailable: state.serviceAvailable,
        codexApp: state.codexApp,
        telemetryEnabled: state.telemetryEnabled,
        autoUpdateEnabled: state.autoUpdateEnabled,
        errorCode: event.errorCode,
      };

    case "signed-out":
      return {
        ...initialLumioState(),
        phase: "signed-out",
        bootstrap: state.bootstrap,
        service: state.service,
        serviceAvailable: state.serviceAvailable,
        codexApp: state.codexApp,
        telemetryEnabled: state.telemetryEnabled,
        autoUpdateEnabled: state.autoUpdateEnabled,
      };
  }
}
```

> 注意：`bootstrapped` 现在会把 `account` / `codexApp` 写进顶层字段，`LumioApp.tsx` 里原先读 `state.bootstrap?.account` 的地方在 Task 9 会改成读顶层字段。本任务只需保证 `npm run check` 通过——若 `LumioApp.tsx` 因 `LumioActions` 新增字段报错，在本任务内做最小修补（补齐字段即可），不做视图重构。

- [ ] **Step 4: Run test to verify it passes**

Run: `cd apps/codex-plus-manager && node --test src/lumio/state.test.ts && npm run check`
Expected: 全部测试 pass（含原有 4 个）；`tsc` 无输出

- [ ] **Step 5: Commit**

```bash
git add apps/codex-plus-manager/src/lumio/types.ts apps/codex-plus-manager/src/lumio/state.ts apps/codex-plus-manager/src/lumio/state.test.ts apps/codex-plus-manager/src/LumioApp.tsx
git commit -m "feat(lumio): extend state machine to the full auth and provisioning event flow"
```

---

### Task 3: 前端表单校验纯函数

**Files:**
- Create: `apps/codex-plus-manager/src/lumio/forms.ts`
- Test: `apps/codex-plus-manager/src/lumio/forms.test.ts`

**Interfaces:**
- Consumes: Task 2 的 `LumioServiceSettings`、`LumioAgreementDocument`
- Produces：
  - `isValidEmail(email: string): boolean`
  - `emailSuffixError(email: string, whitelist: string[]): string | null` — 返回错误码或 `null`
  - `formatEmailSuffixHint(whitelist: string[]): string | null`
  - `sanitizeVerifyCode(raw: string): string` — 只保留数字，截断到 6 位
  - `passwordStrength(password: string): "weak" | "medium" | "strong"`
  - `registerFormError(input: RegisterFormInput, settings: LumioServiceSettings): string | null`
  - 类型 `RegisterFormInput { email; verifyCode; password; confirmPassword; acceptedDocumentIds: string[] }`

- [ ] **Step 1: Write the failing test**

创建 `apps/codex-plus-manager/src/lumio/forms.test.ts`：

```ts
import assert from "node:assert/strict";
import test from "node:test";

import {
  emailSuffixError,
  formatEmailSuffixHint,
  isValidEmail,
  passwordStrength,
  registerFormError,
  sanitizeVerifyCode,
} from "./forms.ts";
import type { RegisterFormInput } from "./forms.ts";
import type { LumioServiceSettings } from "./state.ts";

const SETTINGS: LumioServiceSettings = {
  registrationEnabled: true,
  emailVerifyEnabled: true,
  emailSuffixWhitelist: ["@example.com", "@lumio.games"],
  passwordResetEnabled: true,
  agreementEnabled: true,
  agreementRevision: "v2026-03",
  agreementDocuments: [
    { id: "terms", title: "服务条款", contentMd: "" },
    { id: "usage-policy", title: "使用政策", contentMd: "" },
  ],
  defaultModel: "gpt-example",
  siteBaseUrl: "https://api.lumio.games",
};

const VALID: RegisterFormInput = {
  email: "user@example.com",
  verifyCode: "123456",
  password: "supersecret",
  confirmPassword: "supersecret",
  acceptedDocumentIds: ["terms", "usage-policy"],
};

test("email validation accepts ordinary addresses and rejects malformed ones", () => {
  assert.equal(isValidEmail("user@example.com"), true);
  assert.equal(isValidEmail("user.name+tag@sub.example.co.uk"), true);
  assert.equal(isValidEmail("user@"), false);
  assert.equal(isValidEmail("user example.com"), false);
  assert.equal(isValidEmail(""), false);
});

test("an empty whitelist allows every suffix", () => {
  assert.equal(emailSuffixError("user@anywhere.dev", []), null);
  assert.equal(formatEmailSuffixHint([]), null);
});

test("a whitelist rejects other suffixes case-insensitively", () => {
  assert.equal(emailSuffixError("user@EXAMPLE.com", ["@example.com"]), null);
  assert.equal(
    emailSuffixError("user@other.dev", ["@example.com"]),
    "AUTH_EMAIL_DOMAIN_NOT_ALLOWED",
  );
});

test("the suffix hint lists every allowed suffix", () => {
  assert.equal(
    formatEmailSuffixHint(["@example.com", "@lumio.games"]),
    "支持的邮箱：@example.com、@lumio.games",
  );
});

test("verify code input keeps at most six digits", () => {
  assert.equal(sanitizeVerifyCode("12a3b4"), "1234");
  assert.equal(sanitizeVerifyCode("1234567"), "123456");
  assert.equal(sanitizeVerifyCode("  12 34  "), "1234");
});

test("password strength grows with length and character variety", () => {
  assert.equal(passwordStrength("abc"), "weak");
  assert.equal(passwordStrength("abcdefgh"), "weak");
  assert.equal(passwordStrength("abcdefg1"), "medium");
  assert.equal(passwordStrength("Abcdefg1!"), "strong");
});

test("a complete form produces no error", () => {
  assert.equal(registerFormError(VALID, SETTINGS), null);
});

test("form validation reports the first blocking problem as a stable code", () => {
  assert.equal(
    registerFormError({ ...VALID, email: "nope" }, SETTINGS),
    "AUTH_EMAIL_DOMAIN_NOT_ALLOWED",
  );
  assert.equal(registerFormError({ ...VALID, verifyCode: "" }, SETTINGS), "AUTH_CODE_REQUIRED");
  assert.equal(registerFormError({ ...VALID, verifyCode: "123" }, SETTINGS), "AUTH_CODE_REQUIRED");
  assert.equal(
    registerFormError({ ...VALID, password: "short", confirmPassword: "short" }, SETTINGS),
    "PASSWORD_TOO_SHORT",
  );
  assert.equal(
    registerFormError({ ...VALID, confirmPassword: "different" }, SETTINGS),
    "PASSWORD_MISMATCH",
  );
  assert.equal(
    registerFormError({ ...VALID, acceptedDocumentIds: ["terms"] }, SETTINGS),
    "AGREEMENTS_NOT_ACCEPTED",
  );
});

test("verification code is not required when the server does not enforce it", () => {
  const relaxed = { ...SETTINGS, emailVerifyEnabled: false };
  assert.equal(registerFormError({ ...VALID, verifyCode: "" }, relaxed), null);
});

test("agreements are not required when the server disables them", () => {
  const relaxed = { ...SETTINGS, agreementEnabled: false };
  assert.equal(registerFormError({ ...VALID, acceptedDocumentIds: [] }, relaxed), null);
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/codex-plus-manager && node --test src/lumio/forms.test.ts`
Expected: FAIL — `Cannot find module './forms.ts'`

- [ ] **Step 3: Write minimal implementation**

创建 `apps/codex-plus-manager/src/lumio/forms.ts`：

```ts
import type { LumioServiceSettings } from "./state.ts";

const EMAIL_PATTERN = /^[^\s@]+@[^\s@.]+(\.[^\s@.]+)+$/;
const MIN_PASSWORD_LENGTH = 8;
const VERIFY_CODE_LENGTH = 6;

export interface RegisterFormInput {
  email: string;
  verifyCode: string;
  password: string;
  confirmPassword: string;
  acceptedDocumentIds: string[];
}

export function isValidEmail(email: string): boolean {
  return EMAIL_PATTERN.test(email.trim());
}

export function emailSuffixError(email: string, whitelist: string[]): string | null {
  if (whitelist.length === 0) return null;
  const normalized = email.trim().toLowerCase();
  const allowed = whitelist.some((suffix) => normalized.endsWith(suffix.trim().toLowerCase()));
  return allowed ? null : "AUTH_EMAIL_DOMAIN_NOT_ALLOWED";
}

export function formatEmailSuffixHint(whitelist: string[]): string | null {
  if (whitelist.length === 0) return null;
  return `支持的邮箱：${whitelist.join("、")}`;
}

export function sanitizeVerifyCode(raw: string): string {
  return raw.replace(/\D/g, "").slice(0, VERIFY_CODE_LENGTH);
}

export function passwordStrength(password: string): "weak" | "medium" | "strong" {
  if (password.length < MIN_PASSWORD_LENGTH) return "weak";
  let variety = 0;
  if (/[a-z]/.test(password)) variety += 1;
  if (/[A-Z]/.test(password)) variety += 1;
  if (/\d/.test(password)) variety += 1;
  if (/[^A-Za-z0-9]/.test(password)) variety += 1;
  if (variety >= 3) return "strong";
  if (variety >= 2) return "medium";
  return "weak";
}

export function registerFormError(
  input: RegisterFormInput,
  settings: LumioServiceSettings,
): string | null {
  if (!isValidEmail(input.email)) return "AUTH_EMAIL_DOMAIN_NOT_ALLOWED";
  const suffixError = emailSuffixError(input.email, settings.emailSuffixWhitelist);
  if (suffixError !== null) return suffixError;
  if (settings.emailVerifyEnabled && input.verifyCode.length !== VERIFY_CODE_LENGTH) {
    return "AUTH_CODE_REQUIRED";
  }
  if (input.password.length < MIN_PASSWORD_LENGTH) return "PASSWORD_TOO_SHORT";
  if (input.password !== input.confirmPassword) return "PASSWORD_MISMATCH";
  if (settings.agreementEnabled) {
    const accepted = new Set(input.acceptedDocumentIds);
    const missing = settings.agreementDocuments.some((doc) => !accepted.has(doc.id));
    if (missing) return "AGREEMENTS_NOT_ACCEPTED";
  }
  return null;
}
```

> `PASSWORD_TOO_SHORT` / `PASSWORD_MISMATCH` / `AGREEMENTS_NOT_ACCEPTED` 是纯客户端字段级校验标识，不是服务端错误码，因此**不进** Task 1 的 `LUMIO_ERROR_COPY`（那张表的域约束测试会拒绝它们）。它们的字段级文案在 Task 6 的注册视图里直接给出：分别为「密码至少 8 位」「两次输入不一致」「请先阅读并勾选全部协议」。

- [ ] **Step 4: Run test to verify it passes**

Run: `cd apps/codex-plus-manager && node --test src/lumio/forms.test.ts && npm run check`
Expected: 10 tests pass

- [ ] **Step 5: Commit**

```bash
git add apps/codex-plus-manager/src/lumio/forms.ts apps/codex-plus-manager/src/lumio/forms.test.ts
git commit -m "feat(lumio): add pure form validation helpers for registration"
```

---

### Task 4: Rust 错误归一化与脱敏器

**Files:**
- Create: `crates/codex-plus-core/src/lumio/errors.rs`
- Modify: `crates/codex-plus-core/src/lumio/mod.rs`（当前内容仅一行 `pub mod product;`）
- Test: 同文件内 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: 无
- Produces:
  - `pub struct LumioError { pub code: String, pub stage: &'static str }`
  - `pub fn normalize_reason(http_status: u16, reason: Option<&str>) -> String`
  - `pub fn network_error_code() -> &'static str`（返回 `"SERVICE_UNAVAILABLE"`）
  - `pub fn redact(input: &str) -> String`

- [ ] **Step 1: Write the failing test**

创建 `crates/codex-plus-core/src/lumio/errors.rs`，先只写测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_reasons_collapse_to_one_code_to_avoid_account_enumeration() {
        assert_eq!(normalize_reason(401, Some("INVALID_CREDENTIALS")), "AUTH_INVALID_CREDENTIALS");
        assert_eq!(normalize_reason(401, Some("INVALID_USER")), "AUTH_INVALID_CREDENTIALS");
    }

    #[test]
    fn verification_code_reasons_map_to_their_ux_codes() {
        assert_eq!(normalize_reason(400, Some("INVALID_VERIFY_CODE")), "AUTH_CODE_INVALID");
        assert_eq!(normalize_reason(429, Some("VERIFY_CODE_MAX_ATTEMPTS")), "AUTH_CODE_INVALID");
        assert_eq!(normalize_reason(429, Some("VERIFY_CODE_TOO_FREQUENT")), "AUTH_CODE_RATE_LIMITED");
        assert_eq!(normalize_reason(400, Some("EMAIL_VERIFY_REQUIRED")), "AUTH_CODE_REQUIRED");
    }

    #[test]
    fn registration_reasons_map_to_their_ux_codes() {
        assert_eq!(normalize_reason(403, Some("REGISTRATION_DISABLED")), "AUTH_REGISTRATION_CLOSED");
        assert_eq!(normalize_reason(400, Some("EMAIL_SUFFIX_NOT_ALLOWED")), "AUTH_EMAIL_DOMAIN_NOT_ALLOWED");
        assert_eq!(normalize_reason(400, Some("EMAIL_RESERVED")), "AUTH_EMAIL_DOMAIN_NOT_ALLOWED");
        assert_eq!(normalize_reason(409, Some("EMAIL_EXISTS")), "AUTH_EMAIL_ALREADY_REGISTERED");
    }

    #[test]
    fn two_factor_reasons_map_to_their_ux_codes() {
        assert_eq!(normalize_reason(400, Some("TOTP_INVALID_CODE")), "AUTH_2FA_INVALID");
        assert_eq!(normalize_reason(429, Some("TOTP_TOO_MANY_ATTEMPTS")), "AUTH_2FA_INVALID");
        assert_eq!(normalize_reason(400, Some("TOTP_NOT_SETUP")), "AUTH_2FA_UNAVAILABLE");
        assert_eq!(normalize_reason(400, Some("TOTP_NOT_ENABLED")), "AUTH_2FA_UNAVAILABLE");
    }

    #[test]
    fn every_token_failure_becomes_a_single_session_expiry_code() {
        for reason in [
            "INVALID_TOKEN",
            "TOKEN_EXPIRED",
            "ACCESS_TOKEN_EXPIRED",
            "TOKEN_REVOKED",
            "REFRESH_TOKEN_INVALID",
            "REFRESH_TOKEN_EXPIRED",
            "REFRESH_TOKEN_REUSED",
            "SESSION_BINDING_MISMATCH",
        ] {
            assert_eq!(normalize_reason(401, Some(reason)), "AUTH_SESSION_EXPIRED", "{reason}");
        }
    }

    #[test]
    fn disabled_accounts_are_distinguishable_from_bad_passwords() {
        assert_eq!(normalize_reason(403, Some("USER_NOT_ACTIVE")), "AUTH_ACCOUNT_DISABLED");
    }

    #[test]
    fn key_provisioning_reasons_collapse_into_the_key_domain() {
        for reason in [
            "GROUP_NOT_ALLOWED",
            "API_KEY_EXISTS",
            "API_KEY_RATE_LIMITED",
            "IDEMPOTENCY_KEY_CONFLICT",
            "IDEMPOTENCY_IN_PROGRESS",
        ] {
            assert_eq!(normalize_reason(409, Some(reason)), "KEY_PROVISION_FAILED", "{reason}");
        }
    }

    #[test]
    fn backend_mode_and_server_faults_read_as_service_unavailable() {
        assert_eq!(normalize_reason(403, Some("BACKEND_MODE_ADMIN_ONLY")), "SERVICE_UNAVAILABLE");
        assert_eq!(normalize_reason(503, Some("SERVICE_UNAVAILABLE")), "SERVICE_UNAVAILABLE");
        assert_eq!(normalize_reason(500, None), "SERVICE_UNAVAILABLE");
        assert_eq!(normalize_reason(502, None), "SERVICE_UNAVAILABLE");
    }

    #[test]
    fn a_rate_limited_response_without_a_reason_still_gets_a_stable_code() {
        assert_eq!(normalize_reason(429, None), "SERVICE_RATE_LIMITED");
    }

    #[test]
    fn unrecognized_reasons_do_not_leak_the_server_string() {
        let code = normalize_reason(418, Some("SOME_BRAND_NEW_SERVER_REASON"));
        assert_eq!(code, "SERVICE_UNAVAILABLE");
        assert!(!code.contains("BRAND_NEW"));
    }

    #[test]
    fn redaction_removes_bearer_tokens_keys_and_emails() {
        let dirty = "Authorization: Bearer eyJhbGciOi.JIUzI1NiJ9.sig key=sk-abcdef0123456789 user@example.com rt_0123456789abcdef";
        let clean = redact(dirty);

        assert!(!clean.contains("eyJhbGciOi"));
        assert!(!clean.contains("sk-abcdef0123456789"));
        assert!(!clean.contains("user@example.com"));
        assert!(!clean.contains("rt_0123456789abcdef"));
        assert!(clean.contains("[redacted]"));
    }

    #[test]
    fn redaction_leaves_ordinary_diagnostics_readable() {
        assert_eq!(redact("stage=prepare-connection code=KEY_PROVISION_FAILED"), "stage=prepare-connection code=KEY_PROVISION_FAILED");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

先在 `crates/codex-plus-core/src/lumio/mod.rs` 加上 `pub mod errors;`，再运行：

Run: `cargo test -p codex-plus-core lumio::errors`
Expected: 编译失败 — `cannot find function normalize_reason`

- [ ] **Step 3: Write minimal implementation**

在 `crates/codex-plus-core/src/lumio/errors.rs` 的测试模块之前加入：

```rust
/// Lumio 面向 UI 的稳定错误码。服务端 reason 与网络故障都先归一化到这里，
/// 原始服务端字符串永不越过这一层。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LumioError {
    pub code: String,
    pub stage: &'static str,
}

impl LumioError {
    pub fn new(code: impl Into<String>, stage: &'static str) -> Self {
        Self {
            code: code.into(),
            stage,
        }
    }
}

pub fn network_error_code() -> &'static str {
    "SERVICE_UNAVAILABLE"
}

pub fn normalize_reason(http_status: u16, reason: Option<&str>) -> String {
    let mapped = match reason.map(str::trim).filter(|value| !value.is_empty()) {
        Some("INVALID_CREDENTIALS" | "INVALID_USER") => "AUTH_INVALID_CREDENTIALS",
        Some("USER_NOT_ACTIVE") => "AUTH_ACCOUNT_DISABLED",
        Some("INVALID_VERIFY_CODE" | "VERIFY_CODE_MAX_ATTEMPTS") => "AUTH_CODE_INVALID",
        Some("VERIFY_CODE_TOO_FREQUENT") => "AUTH_CODE_RATE_LIMITED",
        Some("EMAIL_VERIFY_REQUIRED") => "AUTH_CODE_REQUIRED",
        Some("REGISTRATION_DISABLED") => "AUTH_REGISTRATION_CLOSED",
        Some("EMAIL_SUFFIX_NOT_ALLOWED" | "EMAIL_RESERVED") => "AUTH_EMAIL_DOMAIN_NOT_ALLOWED",
        Some("EMAIL_EXISTS") => "AUTH_EMAIL_ALREADY_REGISTERED",
        Some("TOTP_INVALID_CODE" | "TOTP_TOO_MANY_ATTEMPTS") => "AUTH_2FA_INVALID",
        Some("TOTP_NOT_SETUP" | "TOTP_NOT_ENABLED" | "TOTP_SETUP_EXPIRED") => {
            "AUTH_2FA_UNAVAILABLE"
        }
        Some(
            "INVALID_TOKEN"
            | "TOKEN_EXPIRED"
            | "ACCESS_TOKEN_EXPIRED"
            | "TOKEN_REVOKED"
            | "TOKEN_TOO_LARGE"
            | "REFRESH_TOKEN_INVALID"
            | "REFRESH_TOKEN_EXPIRED"
            | "REFRESH_TOKEN_REUSED"
            | "SESSION_BINDING_MISMATCH",
        ) => "AUTH_SESSION_EXPIRED",
        Some(
            "GROUP_NOT_ALLOWED"
            | "API_KEY_EXISTS"
            | "API_KEY_RATE_LIMITED"
            | "API_KEY_TOO_SHORT"
            | "API_KEY_INVALID_CHARS"
            | "INVALID_IP_PATTERN"
            | "IDEMPOTENCY_KEY_REQUIRED"
            | "IDEMPOTENCY_KEY_CONFLICT"
            | "IDEMPOTENCY_IN_PROGRESS"
            | "IDEMPOTENCY_STORE_UNAVAILABLE",
        ) => "KEY_PROVISION_FAILED",
        _ => "",
    };

    if !mapped.is_empty() {
        return mapped.to_string();
    }

    if http_status == 429 {
        return "SERVICE_RATE_LIMITED".to_string();
    }
    if http_status == 401 {
        return "AUTH_SESSION_EXPIRED".to_string();
    }
    network_error_code().to_string()
}

/// 在任何字符串越过 IPC 边界或进入日志前调用。覆盖 Bearer 令牌、`sk-` 类
/// Key、`rt_` 刷新令牌、JWT 与邮箱。
pub fn redact(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for token in input.split_inclusive(char::is_whitespace) {
        let (word, trailing) = split_trailing_whitespace(token);
        if is_secret_like(word) {
            out.push_str("[redacted]");
        } else {
            out.push_str(word);
        }
        out.push_str(trailing);
    }
    out
}

fn split_trailing_whitespace(token: &str) -> (&str, &str) {
    let end = token.trim_end().len();
    token.split_at(end)
}

fn is_secret_like(word: &str) -> bool {
    let candidate = word.rsplit(['=', ':']).next().unwrap_or(word);
    if candidate.is_empty() {
        return false;
    }
    if candidate.contains('@') && candidate.contains('.') {
        return true;
    }
    if candidate.starts_with("sk-") || candidate.starts_with("rt_") {
        return true;
    }
    // JWT: 三段 base64url，用 `.` 分隔。
    let segments: Vec<&str> = candidate.split('.').collect();
    if segments.len() == 3 && segments.iter().all(|segment| segment.len() >= 8) {
        return true;
    }
    false
}
```

由于 `redact` 是按空白切词的，`"Authorization: Bearer <token>"` 中的 `Bearer` 后跟的令牌会作为独立词命中 JWT 规则。测试里的 `key=sk-...` 由 `rsplit(['=', ':'])` 取到 `sk-...` 后命中前缀规则。

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p codex-plus-core lumio::errors && cargo fmt --all -- --check`
Expected: 12 tests pass；fmt 无输出

- [ ] **Step 5: Commit**

```bash
git add crates/codex-plus-core/src/lumio/errors.rs crates/codex-plus-core/src/lumio/mod.rs
git commit -m "feat(lumio): normalize sub2api reasons into stable error codes with redaction"
```

---

### Task 5: Sub2API HTTP 客户端

**Files:**
- Create: `crates/codex-plus-core/src/lumio/api.rs`
- Modify: `crates/codex-plus-core/src/lumio/mod.rs`（加 `pub mod api;`）
- Test: 同文件内 `#[cfg(test)] mod tests`，用 `wiremock`（已是 `codex-plus-core` 的 dev-dependency）

**Interfaces:**
- Consumes: Task 4 的 `normalize_reason`、`network_error_code`；`crate::lumio::product::API_BASE_URL`
- Produces：
  - `pub struct LumioApiClient { .. }`，`LumioApiClient::new(base_url: &str) -> anyhow::Result<Self>`
  - `pub async fn public_settings(&self) -> Result<PublicSettings, String>`
  - `pub async fn send_verify_code(&self, email: &str) -> Result<u32, String>`（返回倒计时秒数）
  - `pub async fn register(&self, req: &RegisterRequest) -> Result<AuthOutcome, String>`
  - `pub async fn login(&self, email: &str, password: &str) -> Result<AuthOutcome, String>`
  - `pub async fn login_two_factor(&self, temp_token: &str, code: &str) -> Result<AuthOutcome, String>`
  - `pub async fn refresh(&self, refresh_token: &str) -> Result<TokenPair, String>`
  - `pub async fn logout(&self, refresh_token: &str) -> Result<(), String>`
  - `pub async fn me(&self, access_token: &str) -> Result<AccountProfile, String>`
  - `pub async fn available_groups(&self, access_token: &str) -> Result<Vec<GroupSummary>, String>`
  - `pub async fn list_keys(&self, access_token: &str, name: &str) -> Result<Vec<ApiKeyRecord>, String>`
  - `pub async fn create_key(&self, access_token: &str, req: &CreateKeyRequest) -> Result<ApiKeyRecord, String>`
  - `pub async fn models(&self, api_key: &str) -> Result<Vec<String>, String>`
  - 全部 `Err` 分支返回的 `String` 都是 Task 4 归一化后的 Lumio 错误码，不含服务端原文。
  - 数据类型：`PublicSettings`、`AgreementDocument`、`RegisterRequest`、`AuthOutcome`（`Tokens(TokenPair, AccountProfile)` 或 `TwoFactorRequired { temp_token: String, masked_email: String }`）、`TokenPair`、`AccountProfile`、`GroupSummary`、`ApiKeyRecord`、`CreateKeyRequest`

- [ ] **Step 1: Write the failing test**

创建 `crates/codex-plus-core/src/lumio/api.rs` 的测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, header, header_exists, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn envelope(data: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "code": 0, "message": "success", "data": data })
    }

    fn failure(code: u16, reason: &str, message: &str) -> serde_json::Value {
        serde_json::json!({ "code": code, "message": message, "reason": reason })
    }

    #[tokio::test]
    async fn public_settings_reads_the_registration_rules() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/settings/public"))
            .respond_with(ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "registration_enabled": true,
                "email_verify_enabled": true,
                "registration_email_suffix_whitelist": ["@example.com"],
                "password_reset_enabled": true,
                "login_agreement_enabled": true,
                "login_agreement_revision": "abc123",
                "login_agreement_documents": [
                    { "id": "terms", "title": "服务条款", "content_md": "# 条款" }
                ],
                "ccswitch_default_model_openai": "gpt-example",
                "site_name": "Lumio"
            }))))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let settings = client.public_settings().await.unwrap();

        assert!(settings.registration_enabled);
        assert!(settings.email_verify_enabled);
        assert_eq!(settings.email_suffix_whitelist, vec!["@example.com".to_string()]);
        assert_eq!(settings.agreement_revision, "abc123");
        assert_eq!(settings.agreement_documents.len(), 1);
        assert_eq!(settings.agreement_documents[0].id, "terms");
        assert_eq!(settings.default_model.as_deref(), Some("gpt-example"));
    }

    #[tokio::test]
    async fn missing_optional_settings_fields_fall_back_safely() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/settings/public"))
            .respond_with(ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "registration_enabled": false
            }))))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let settings = client.public_settings().await.unwrap();

        assert!(!settings.registration_enabled);
        assert!(settings.email_suffix_whitelist.is_empty());
        assert!(settings.agreement_documents.is_empty());
        assert_eq!(settings.default_model, None);
    }

    #[tokio::test]
    async fn login_returns_tokens_and_profile() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/login"))
            .and(body_json(serde_json::json!({
                "email": "user@example.com",
                "password": "supersecret"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "access_token": "header.payload.signature",
                "refresh_token": "rt_abc",
                "expires_in": 3600,
                "token_type": "Bearer",
                "user": { "id": 7, "email": "user@example.com", "balance": 12.5, "status": "active" }
            }))))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let outcome = client.login("user@example.com", "supersecret").await.unwrap();

        match outcome {
            AuthOutcome::Tokens { tokens, profile } => {
                assert_eq!(tokens.access_token, "header.payload.signature");
                assert_eq!(tokens.refresh_token, "rt_abc");
                assert_eq!(profile.email, "user@example.com");
                assert_eq!(profile.balance, 12.5);
            }
            other => panic!("expected tokens, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn login_surfaces_the_two_factor_challenge_as_a_success_variant() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "requires_2fa": true,
                "temp_token": "tmp_123",
                "user_email_masked": "u***@example.com"
            }))))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let outcome = client.login("user@example.com", "supersecret").await.unwrap();

        match outcome {
            AuthOutcome::TwoFactorRequired { temp_token, masked_email } => {
                assert_eq!(temp_token, "tmp_123");
                assert_eq!(masked_email, "u***@example.com");
            }
            other => panic!("expected a two-factor challenge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn bad_credentials_become_a_normalized_code() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/login"))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_json(failure(401, "INVALID_CREDENTIALS", "invalid email or password")),
            )
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let error = client.login("user@example.com", "nope").await.unwrap_err();

        assert_eq!(error, "AUTH_INVALID_CREDENTIALS");
    }

    #[tokio::test]
    async fn the_rate_limiter_response_shape_is_handled_even_without_an_envelope() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/send-verify-code"))
            .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
                "error": "rate limit exceeded",
                "message": "Too many requests, please try again later"
            })))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let error = client.send_verify_code("user@example.com").await.unwrap_err();

        assert_eq!(error, "SERVICE_RATE_LIMITED");
    }

    #[tokio::test]
    async fn send_verify_code_returns_the_server_countdown() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/send-verify-code"))
            .respond_with(ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "message": "Verification code sent successfully",
                "countdown": 60
            }))))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        assert_eq!(client.send_verify_code("user@example.com").await.unwrap(), 60);
    }

    #[tokio::test]
    async fn two_factor_login_sends_the_temp_token_and_code() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/login/2fa"))
            .and(body_json(serde_json::json!({
                "temp_token": "tmp_123",
                "totp_code": "654321"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "access_token": "header.payload.signature",
                "refresh_token": "rt_abc",
                "token_type": "Bearer",
                "user": { "id": 7, "email": "user@example.com", "balance": 0.0, "status": "active" }
            }))))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let outcome = client.login_two_factor("tmp_123", "654321").await.unwrap();

        assert!(matches!(outcome, AuthOutcome::Tokens { .. }));
    }

    #[tokio::test]
    async fn authenticated_requests_carry_the_bearer_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/auth/me"))
            .and(header("authorization", "Bearer access-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "id": 7,
                "email": "user@example.com",
                "balance": 3.25,
                "status": "active"
            }))))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let profile = client.me("access-token").await.unwrap();

        assert_eq!(profile.balance, 3.25);
    }

    #[tokio::test]
    async fn key_listing_filters_by_the_reserved_desktop_name() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/keys"))
            .and(query_param("search", "Lumio Codex Desktop"))
            .respond_with(ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "items": [
                    {
                        "id": 1,
                        "name": "Lumio Codex Desktop",
                        "key": "sk-existing",
                        "status": "active",
                        "group_id": 3,
                        "created_at": "2026-01-01T00:00:00Z"
                    }
                ],
                "total": 1,
                "page": 1,
                "page_size": 20,
                "pages": 1
            }))))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let keys = client.list_keys("access-token", "Lumio Codex Desktop").await.unwrap();

        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].name, "Lumio Codex Desktop");
        assert_eq!(keys[0].status, "active");
    }

    #[tokio::test]
    async fn key_creation_always_sends_an_idempotency_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/keys"))
            .and(header_exists("idempotency-key"))
            .respond_with(ResponseTemplate::new(201).set_body_json(envelope(serde_json::json!({
                "id": 2,
                "name": "Lumio Codex Desktop",
                "key": "sk-created",
                "status": "active",
                "group_id": 3,
                "created_at": "2026-02-01T00:00:00Z"
            }))))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let request = CreateKeyRequest {
            name: "Lumio Codex Desktop".to_string(),
            group_id: Some(3),
        };
        let created = client.create_key("access-token", &request).await.unwrap();

        assert_eq!(created.key, "sk-created");
    }

    #[tokio::test]
    async fn a_dead_server_reports_service_unavailable_rather_than_a_transport_error() {
        let client = LumioApiClient::new("http://127.0.0.1:1").unwrap();
        assert_eq!(client.public_settings().await.unwrap_err(), "SERVICE_UNAVAILABLE");
    }

    #[tokio::test]
    async fn malformed_success_bodies_do_not_panic() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/auth/me"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        assert_eq!(client.me("access-token").await.unwrap_err(), "SERVICE_UNAVAILABLE");
    }

    #[tokio::test]
    async fn the_model_catalog_uses_api_key_auth_and_returns_model_ids() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", "Bearer sk-desktop"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "object": "list",
                "data": [{ "id": "gpt-example" }, { "id": "gpt-example-mini" }]
            })))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let models = client.models("sk-desktop").await.unwrap();

        assert_eq!(models, vec!["gpt-example".to_string(), "gpt-example-mini".to_string()]);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p codex-plus-core lumio::api`
Expected: 编译失败 — `cannot find type LumioApiClient`

- [ ] **Step 3: Write minimal implementation**

在 `api.rs` 中实现。要点：

1. `LumioApiClient` 持有 `reqwest::Client`（用 `Client::builder().user_agent(...).connect_timeout(5s).timeout(20s)` 自建，**不**复用 `http_client::proxied_client`——那个没有超时）与 `base_url: String`（去掉末尾 `/`）。
2. 统一的 envelope 解析函数：

```rust
#[derive(Debug, serde::Deserialize)]
struct Envelope<T> {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    reason: Option<String>,
    data: Option<T>,
}

async fn read_envelope<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, String> {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let reason = serde_json::from_str::<Envelope<serde_json::Value>>(&body)
            .ok()
            .and_then(|envelope| envelope.reason);
        return Err(super::errors::normalize_reason(status.as_u16(), reason.as_deref()));
    }
    let envelope: Envelope<T> = serde_json::from_str(&body)
        .map_err(|_| super::errors::network_error_code().to_string())?;
    if envelope.code != 0 {
        return Err(super::errors::normalize_reason(
            status.as_u16(),
            envelope.reason.as_deref(),
        ));
    }
    envelope
        .data
        .ok_or_else(|| super::errors::network_error_code().to_string())
}
```

3. 每次网络调用的 `send()` 失败都映射为 `network_error_code().to_string()`，不得把 `reqwest::Error` 的 Display 透传出去（其中可能含 URL 与查询串）。
4. `PublicSettings` 用 `#[serde(default)]` 逐字段容错，字段名严格照服务端 json tag：`registration_enabled`、`email_verify_enabled`、`registration_email_suffix_whitelist`、`password_reset_enabled`、`login_agreement_enabled`、`login_agreement_revision`、`login_agreement_documents`（元素 `id` / `title` / `content_md`）、`ccswitch_default_model_openai`（空串归一化为 `None`，映射到 `default_model`）。
5. `AuthOutcome` 反序列化策略：先解析成 `serde_json::Value`，若 `requires_2fa == true` 则走 `TwoFactorRequired`，否则解析为 `AuthResponse`。
6. `models()` 打的是 `/v1/models`（**没有** `/api` 前缀），用 API Key 做 Bearer，且该端点**不走** envelope，直接是 `{"object":"list","data":[{"id":...}]}`。
7. `create_key` 每次调用生成 `uuid::Uuid::new_v4()` 作为 `Idempotency-Key` header 值（`uuid` 已是 `codex-plus-core` 依赖）。
8. `list_keys` 用 query `search=<name>`、`page_size=100`，解析分页 envelope 的 `items`。
9. `AccountProfile` 只取 `id` / `email` / `balance` / `status`，其余用户字段一律忽略（不 deserialize 无关字段，减少契约耦合）。

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p codex-plus-core lumio::api && cargo fmt --all -- --check`
Expected: 14 tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/codex-plus-core/src/lumio/api.rs crates/codex-plus-core/src/lumio/mod.rs
git commit -m "feat(lumio): add sub2api http client with normalized error codes"
```

---

### Task 6: 本地凭据存储

**Files:**
- Create: `crates/codex-plus-core/src/lumio/credentials.rs`
- Modify: `crates/codex-plus-core/src/lumio/mod.rs`（加 `pub mod credentials;`）
- Test: 同文件内 `#[cfg(test)] mod tests`，用 `tempfile`

**Interfaces:**
- Consumes: `crate::settings::atomic_write`
- Produces：
  - `pub enum CredentialStatus { Present, Missing, Invalid }`（`Serialize` 为 `"present" | "missing" | "invalid"`）
  - `pub struct StoredCredentials { pub access_token: String, pub refresh_token: String, pub api_key: Option<String>, pub email: String }`
  - `pub struct CredentialStore { root: PathBuf }`
  - `CredentialStore::new(root: impl Into<PathBuf>) -> Self`
  - `CredentialStore::default_store() -> anyhow::Result<Self>`（基于 `product::state_dir()`）
  - `pub fn status(&self) -> CredentialStatus`
  - `pub fn load(&self) -> Option<StoredCredentials>`
  - `pub fn save(&self, credentials: &StoredCredentials) -> Result<(), String>`（Err 为 `"KEY_STORAGE_UNAVAILABLE"`）
  - `pub fn clear(&self) -> Result<(), String>`

**关键约束**：本模块是 ADR `.spec/decisions/0001-lumio-credentials-local-file.md` 的落点。`StoredCredentials` **不得** derive `Serialize` 之外的任何向前端暴露的路径；Tauri 命令层只能返回 `CredentialStatus`。

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> StoredCredentials {
        StoredCredentials {
            access_token: "header.payload.signature".to_string(),
            refresh_token: "rt_abc".to_string(),
            api_key: Some("sk-desktop".to_string()),
            email: "user@example.com".to_string(),
        }
    }

    #[test]
    fn an_empty_store_reports_missing() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());

        assert_eq!(store.status(), CredentialStatus::Missing);
        assert!(store.load().is_none());
    }

    #[test]
    fn saved_credentials_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());
        store.save(&sample()).unwrap();

        assert_eq!(store.status(), CredentialStatus::Present);
        let loaded = store.load().unwrap();
        assert_eq!(loaded.access_token, "header.payload.signature");
        assert_eq!(loaded.api_key.as_deref(), Some("sk-desktop"));
        assert_eq!(loaded.email, "user@example.com");
    }

    #[test]
    fn a_corrupted_file_reports_invalid_instead_of_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());
        store.save(&sample()).unwrap();
        std::fs::write(store.path(), b"{ not json").unwrap();

        assert_eq!(store.status(), CredentialStatus::Invalid);
        assert!(store.load().is_none());
    }

    #[test]
    fn a_file_from_a_future_schema_version_reports_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(store.path(), br#"{"schema_version":99,"email":"a@b.c"}"#).unwrap();

        assert_eq!(store.status(), CredentialStatus::Invalid);
    }

    #[test]
    fn clearing_removes_the_file_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());
        store.save(&sample()).unwrap();

        store.clear().unwrap();
        assert_eq!(store.status(), CredentialStatus::Missing);
        store.clear().unwrap();
        assert_eq!(store.status(), CredentialStatus::Missing);
    }

    #[test]
    fn saving_replaces_the_previous_record_rather_than_appending() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());
        store.save(&sample()).unwrap();
        store.save(&StoredCredentials { api_key: None, ..sample() }).unwrap();

        assert_eq!(store.load().unwrap().api_key, None);
        let raw = std::fs::read_to_string(store.path()).unwrap();
        assert_eq!(raw.matches("\"email\"").count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn the_credential_file_is_only_readable_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());
        store.save(&sample()).unwrap();

        let mode = std::fs::metadata(store.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "unexpected mode {:o}", mode & 0o777);
    }

    #[test]
    fn the_status_serializes_to_the_three_values_the_ui_expects() {
        assert_eq!(serde_json::to_string(&CredentialStatus::Present).unwrap(), "\"present\"");
        assert_eq!(serde_json::to_string(&CredentialStatus::Missing).unwrap(), "\"missing\"");
        assert_eq!(serde_json::to_string(&CredentialStatus::Invalid).unwrap(), "\"invalid\"");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p codex-plus-core lumio::credentials`
Expected: 编译失败 — `cannot find type CredentialStore`

- [ ] **Step 3: Write minimal implementation**

要点：

- 文件名固定 `credentials.json`，位于 `root`；`path()` 为 `pub fn path(&self) -> PathBuf`。
- 磁盘格式：`{"schema_version":1,"email":...,"access_token":...,"refresh_token":...,"api_key":...}`。反序列化时校验 `schema_version == 1`，否则 `Invalid`。
- `save` 走 `crate::settings::atomic_write`，写完在 Unix 上 `std::fs::set_permissions(path, Permissions::from_mode(0o600))`。注意 `atomic_write` 是 rename 语义，权限要在 rename **之后**设置到最终路径上。
- 任何 IO / 序列化失败都返回 `Err("KEY_STORAGE_UNAVAILABLE".to_string())`，绝不把 `io::Error` 的路径信息透传。
- `CredentialStatus` 用 `#[derive(Serialize)] #[serde(rename_all = "lowercase")]`。

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p codex-plus-core lumio::credentials && cargo fmt --all -- --check`
Expected: 8 tests pass（Unix 上 8 个，Windows 上 7 个）

- [ ] **Step 5: Commit**

```bash
git add crates/codex-plus-core/src/lumio/credentials.rs crates/codex-plus-core/src/lumio/mod.rs
git commit -m "feat(lumio): store credentials in an owner-only local file behind a three-state api"
```

---

### Task 7: 官方 Codex 配置接管

**Files:**
- Create: `crates/codex-plus-core/src/lumio/config_takeover.rs`
- Modify: `crates/codex-plus-core/src/lumio/mod.rs`（加 `pub mod config_takeover;`）
- Test: 同文件内 `#[cfg(test)] mod tests`，全部用 `tempfile` 传入自定义 `codex_home`，**不得**依赖 `CODEX_HOME` 环境变量（会与其他测试串扰）

**Interfaces:**
- Consumes: `crate::settings::atomic_write`
- Produces：
  - `pub struct TakeoverRequest { pub model: String, pub api_key: String, pub base_url: String }`
  - `pub struct TakeoverRecord { pub applied_at: String, pub config_sha256: String, pub auth_sha256: String, pub model: String }`
  - `pub fn apply_takeover(codex_home: &Path, state_dir: &Path, request: &TakeoverRequest) -> Result<TakeoverRecord, String>`
  - `pub fn check_takeover(codex_home: &Path, state_dir: &Path) -> TakeoverHealth`
  - `pub enum TakeoverHealth { NotApplied, Healthy, Conflicted { error_code: String } }`
  - `pub fn restore(codex_home: &Path, state_dir: &Path) -> Result<(), String>`
  - 全部 `Err(String)` 为 `CODEX_CONFIG_WRITE_FAILED` / `CODEX_CONFIG_CONFLICT` / `CODEX_RESTORE_FAILED` 之一

**所有权模型（只写这些，其他一律不碰）**：`config.toml` 根键 `model`、根键 `model_provider`、表 `model_providers.lumio`；以及 `auth.json` 的 `OPENAI_API_KEY` 字段。快照同时覆盖 `config.toml` 与 `auth.json`，并记录「文件原本是否存在」。

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> TakeoverRequest {
        TakeoverRequest {
            model: "gpt-example".to_string(),
            api_key: "sk-desktop".to_string(),
            base_url: "https://api.lumio.games/v1".to_string(),
        }
    }

    struct Fixture {
        _root: tempfile::TempDir,
        codex_home: std::path::PathBuf,
        state_dir: std::path::PathBuf,
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let codex_home = root.path().join("codex-home");
        let state_dir = root.path().join("state");
        std::fs::create_dir_all(&codex_home).unwrap();
        std::fs::create_dir_all(&state_dir).unwrap();
        Fixture { _root: root, codex_home, state_dir }
    }

    #[test]
    fn takeover_writes_only_the_fields_lumio_owns() {
        let fx = fixture();
        std::fs::write(
            fx.codex_home.join("config.toml"),
            "model = \"user-choice\"\n\n[mcp_servers.mine]\ncommand = \"keep-me\"\n\n[projects.\"/tmp/x\"]\ntrust_level = \"trusted\"\n",
        )
        .unwrap();

        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();

        let written = std::fs::read_to_string(fx.codex_home.join("config.toml")).unwrap();
        assert!(written.contains("gpt-example"));
        assert!(written.contains("keep-me"), "user-owned section was dropped:\n{written}");
        assert!(written.contains("trust_level"), "user projects were dropped:\n{written}");
    }

    #[test]
    fn takeover_snapshots_the_original_bytes_before_the_first_write() {
        let fx = fixture();
        let original = "model = \"user-choice\"\n";
        std::fs::write(fx.codex_home.join("config.toml"), original).unwrap();

        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();
        restore(&fx.codex_home, &fx.state_dir).unwrap();

        assert_eq!(
            std::fs::read_to_string(fx.codex_home.join("config.toml")).unwrap(),
            original
        );
    }

    #[test]
    fn the_snapshot_is_taken_once_and_survives_a_second_takeover() {
        let fx = fixture();
        let original = "model = \"user-choice\"\n";
        std::fs::write(fx.codex_home.join("config.toml"), original).unwrap();

        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();
        let second = TakeoverRequest { model: "gpt-other".to_string(), ..request() };
        apply_takeover(&fx.codex_home, &fx.state_dir, &second).unwrap();
        restore(&fx.codex_home, &fx.state_dir).unwrap();

        assert_eq!(
            std::fs::read_to_string(fx.codex_home.join("config.toml")).unwrap(),
            original,
            "the second takeover overwrote the pre-takeover snapshot"
        );
    }

    #[test]
    fn restoring_removes_files_that_did_not_exist_before_takeover() {
        let fx = fixture();

        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();
        assert!(fx.codex_home.join("config.toml").exists());

        restore(&fx.codex_home, &fx.state_dir).unwrap();
        assert!(!fx.codex_home.join("config.toml").exists());
        assert!(!fx.codex_home.join("auth.json").exists());
    }

    #[test]
    fn health_reports_not_applied_before_any_takeover() {
        let fx = fixture();
        assert!(matches!(check_takeover(&fx.codex_home, &fx.state_dir), TakeoverHealth::NotApplied));
    }

    #[test]
    fn health_is_clean_right_after_a_takeover() {
        let fx = fixture();
        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();

        assert!(matches!(check_takeover(&fx.codex_home, &fx.state_dir), TakeoverHealth::Healthy));
    }

    #[test]
    fn an_external_edit_after_takeover_is_reported_as_a_conflict() {
        let fx = fixture();
        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();
        std::fs::write(fx.codex_home.join("config.toml"), "model = \"someone-else\"\n").unwrap();

        match check_takeover(&fx.codex_home, &fx.state_dir) {
            TakeoverHealth::Conflicted { error_code } => {
                assert_eq!(error_code, "CODEX_CONFIG_CONFLICT");
            }
            other => panic!("expected a conflict, got {other:?}"),
        }
    }

    #[test]
    fn deleting_the_managed_config_after_takeover_is_also_a_conflict() {
        let fx = fixture();
        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();
        std::fs::remove_file(fx.codex_home.join("config.toml")).unwrap();

        assert!(matches!(
            check_takeover(&fx.codex_home, &fx.state_dir),
            TakeoverHealth::Conflicted { .. }
        ));
    }

    #[test]
    fn the_api_key_lands_in_the_official_auth_file_and_is_removed_on_restore() {
        let fx = fixture();
        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();

        let auth = std::fs::read_to_string(fx.codex_home.join("auth.json")).unwrap();
        assert!(auth.contains("sk-desktop"));

        restore(&fx.codex_home, &fx.state_dir).unwrap();
        assert!(!fx.codex_home.join("auth.json").exists());
    }

    #[test]
    fn restoring_keeps_unrelated_auth_fields_that_existed_before_takeover() {
        let fx = fixture();
        std::fs::write(
            fx.codex_home.join("auth.json"),
            r#"{"OPENAI_API_KEY":"user-key","tokens":{"id_token":"keep"}}"#,
        )
        .unwrap();

        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();
        restore(&fx.codex_home, &fx.state_dir).unwrap();

        let auth = std::fs::read_to_string(fx.codex_home.join("auth.json")).unwrap();
        assert!(auth.contains("user-key"));
        assert!(auth.contains("keep"));
        assert!(!auth.contains("sk-desktop"));
    }

    #[cfg(unix)]
    #[test]
    fn the_auth_file_written_by_takeover_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let fx = fixture();
        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();

        let mode = std::fs::metadata(fx.codex_home.join("auth.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn restore_without_a_snapshot_reports_a_stable_error_code() {
        let fx = fixture();
        assert_eq!(
            restore(&fx.codex_home, &fx.state_dir).unwrap_err(),
            "CODEX_RESTORE_FAILED"
        );
    }

    #[test]
    fn invalid_existing_toml_fails_without_destroying_the_users_file() {
        let fx = fixture();
        let broken = "this is [not valid toml\n";
        std::fs::write(fx.codex_home.join("config.toml"), broken).unwrap();

        let error = apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap_err();

        assert_eq!(error, "CODEX_CONFIG_WRITE_FAILED");
        assert_eq!(
            std::fs::read_to_string(fx.codex_home.join("config.toml")).unwrap(),
            broken
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p codex-plus-core lumio::config_takeover`
Expected: 编译失败 — `cannot find function apply_takeover`

- [ ] **Step 3: Write minimal implementation**

要点：

- 快照落 `state_dir/takeover/`：`config.toml.snapshot`、`auth.json.snapshot`、`manifest.json`。manifest 记录 `config_existed: bool`、`auth_existed: bool`、`config_sha256`（**接管后**的哈希，用于外部修改检测）、`auth_sha256`、`applied_at`、`model`。
- 快照只在 `manifest.json` 不存在时创建；已存在则复用（这是 `the_snapshot_is_taken_once` 的保证）。
- TOML 编辑用 `toml_edit::DocumentMut`：`doc["model"] = value(model)`；`doc["model_provider"] = value("lumio")`；`doc["model_providers"]["lumio"]` 下设 `name` / `base_url` / `wire_api = "responses"` / `env_key`。解析失败直接返回 `Err("CODEX_CONFIG_WRITE_FAILED")`，**在写任何东西之前**返回，保证用户文件不被破坏。
- `auth.json` 用 `serde_json::Value` 读入（不存在则 `{}`），只设 `OPENAI_API_KEY`，其余键原样保留。恢复时若快照记录该文件原本存在，就写回快照字节；否则删除文件。
- 写入统一走 `crate::settings::atomic_write`，写完在 Unix 上把 `auth.json` 设为 `0o600`。
- SHA-256：`use sha2::{Digest, Sha256}; format!("{:x}", Sha256::digest(bytes))`。
- `check_takeover`：无 manifest → `NotApplied`；文件缺失或哈希不等于 manifest 记录 → `Conflicted { error_code: "CODEX_CONFIG_CONFLICT" }`；否则 `Healthy`。
- `TakeoverHealth` 需要 `#[derive(Debug)]`（测试里用了 `{other:?}`）。

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p codex-plus-core lumio::config_takeover && cargo fmt --all -- --check`
Expected: 13 tests pass（Unix）

- [ ] **Step 5: Commit**

```bash
git add crates/codex-plus-core/src/lumio/config_takeover.rs crates/codex-plus-core/src/lumio/mod.rs
git commit -m "feat(lumio): take over only lumio-owned codex config fields with snapshot and restore"
```

---

### Task 8: 账户编排与桌面 Key 初始化

**Files:**
- Create: `crates/codex-plus-core/src/lumio/account.rs`
- Modify: `crates/codex-plus-core/src/lumio/mod.rs`（加 `pub mod account;`）
- Test: 同文件内 `#[cfg(test)] mod tests`，用 `wiremock`

**Interfaces:**
- Consumes: Task 5 的 `LumioApiClient` 全部方法与类型、Task 6 的 `CredentialStore`
- Produces：
  - `pub async fn ensure_desktop_key(client: &LumioApiClient, access_token: &str) -> Result<String, String>` —— 返回 API Key 明文，只在进程内流转
  - `pub fn select_group(groups: &[GroupSummary]) -> Option<i64>`
  - `pub fn is_reusable(key: &ApiKeyRecord, allowed_group_ids: &[i64]) -> bool`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::lumio::api::{ApiKeyRecord, GroupSummary, LumioApiClient};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn key(name: &str, status: &str, group_id: Option<i64>, created_at: &str) -> ApiKeyRecord {
        ApiKeyRecord {
            id: 1,
            name: name.to_string(),
            key: "sk-x".to_string(),
            status: status.to_string(),
            group_id,
            expires_at: None,
            created_at: created_at.to_string(),
        }
    }

    #[test]
    fn only_active_keys_in_an_allowed_group_are_reusable() {
        assert!(is_reusable(&key("Lumio Codex Desktop", "active", Some(3), "2026-01-01"), &[3]));
        assert!(!is_reusable(&key("Lumio Codex Desktop", "disabled", Some(3), "2026-01-01"), &[3]));
        assert!(!is_reusable(&key("Lumio Codex Desktop", "active", Some(9), "2026-01-01"), &[3]));
        assert!(!is_reusable(&key("Other Key", "active", Some(3), "2026-01-01"), &[3]));
    }

    #[test]
    fn a_key_with_no_group_is_reusable_when_the_account_has_no_group_restriction() {
        assert!(is_reusable(&key("Lumio Codex Desktop", "active", None, "2026-01-01"), &[]));
    }

    #[test]
    fn group_selection_prefers_the_first_available_group() {
        let groups = vec![
            GroupSummary { id: 5, name: "beta".to_string() },
            GroupSummary { id: 3, name: "default".to_string() },
        ];
        assert_eq!(select_group(&groups), Some(5));
        assert_eq!(select_group(&[]), None);
    }

    #[tokio::test]
    async fn an_existing_active_key_is_reused_without_creating_another() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/groups/available"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0, "message": "success",
                "data": [{ "id": 3, "name": "default" }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/keys"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0, "message": "success",
                "data": { "items": [{
                    "id": 1, "name": "Lumio Codex Desktop", "key": "sk-existing",
                    "status": "active", "group_id": 3, "created_at": "2026-01-01T00:00:00Z"
                }], "total": 1, "page": 1, "page_size": 100, "pages": 1 }
            })))
            .mount(&server)
            .await;
        // 未 mount POST /api/v1/keys —— 若实现试图创建，wiremock 会以 404 让测试失败。

        let client = LumioApiClient::new(&server.uri()).unwrap();
        let resolved = ensure_desktop_key(&client, "access-token").await.unwrap();

        assert_eq!(resolved, "sk-existing");
    }

    #[tokio::test]
    async fn the_oldest_reusable_key_wins_when_several_share_the_reserved_name() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/groups/available"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0, "message": "success", "data": [{ "id": 3, "name": "default" }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/keys"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0, "message": "success",
                "data": { "items": [
                    { "id": 2, "name": "Lumio Codex Desktop", "key": "sk-newer",
                      "status": "active", "group_id": 3, "created_at": "2026-05-01T00:00:00Z" },
                    { "id": 1, "name": "Lumio Codex Desktop", "key": "sk-older",
                      "status": "active", "group_id": 3, "created_at": "2026-01-01T00:00:00Z" }
                ], "total": 2, "page": 1, "page_size": 100, "pages": 1 }
            })))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        assert_eq!(ensure_desktop_key(&client, "access-token").await.unwrap(), "sk-older");
    }

    #[tokio::test]
    async fn a_key_is_created_when_none_is_reusable() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/groups/available"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0, "message": "success", "data": [{ "id": 3, "name": "default" }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/keys"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0, "message": "success",
                "data": { "items": [{
                    "id": 1, "name": "Lumio Codex Desktop", "key": "sk-dead",
                    "status": "disabled", "group_id": 3, "created_at": "2026-01-01T00:00:00Z"
                }], "total": 1, "page": 1, "page_size": 100, "pages": 1 }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/keys"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "code": 0, "message": "success",
                "data": { "id": 9, "name": "Lumio Codex Desktop", "key": "sk-fresh",
                          "status": "active", "group_id": 3, "created_at": "2026-08-01T00:00:00Z" }
            })))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        assert_eq!(ensure_desktop_key(&client, "access-token").await.unwrap(), "sk-fresh");
    }

    #[tokio::test]
    async fn a_rejected_creation_surfaces_the_key_domain_error_code() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/groups/available"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0, "message": "success", "data": []
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/keys"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0, "message": "success",
                "data": { "items": [], "total": 0, "page": 1, "page_size": 100, "pages": 1 }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/keys"))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "code": 403, "message": "group not allowed", "reason": "GROUP_NOT_ALLOWED"
            })))
            .mount(&server)
            .await;

        let client = LumioApiClient::new(&server.uri()).unwrap();
        assert_eq!(
            ensure_desktop_key(&client, "access-token").await.unwrap_err(),
            "KEY_PROVISION_FAILED"
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p codex-plus-core lumio::account`
Expected: 编译失败 — `cannot find function ensure_desktop_key`

- [ ] **Step 3: Write minimal implementation**

`ensure_desktop_key` 算法（对应设计文档「API Key 初始化」）：

1. `available_groups()` → `select_group()` 得到首选分组（`None` 表示不限制）。
2. `list_keys(access_token, product::DESKTOP_KEY_NAME)`。
3. 过滤 `is_reusable`（名称严格等于 `DESKTOP_KEY_NAME`、`status == "active"`、`expires_at` 为空或在未来、`group_id` 在允许集合内或允许集合为空），按 `created_at` 升序取第一个。
4. 没有可复用的 → `create_key`，请求体 `{ name: DESKTOP_KEY_NAME, group_id }`，客户端自动带 `Idempotency-Key`。
5. 创建后**以服务端返回的记录为准**返回其 `key`。
6. 任何一步失败：`list_keys` / `create_key` 的错误码已由 Task 5 归一化，直接透传；若返回的 key 为空串则返回 `Err("KEY_PROVISION_FAILED")`。

`select_group` 取 `groups.first().map(|g| g.id)`（服务端返回顺序即优先级）。

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p codex-plus-core lumio::account && cargo fmt --all -- --check`
Expected: 7 tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/codex-plus-core/src/lumio/account.rs crates/codex-plus-core/src/lumio/mod.rs
git commit -m "feat(lumio): reuse or create the reserved desktop key during provisioning"
```

---

### Task 9: 无注入启动与浏览器跳转

**Files:**
- Create: `crates/codex-plus-core/src/lumio/launch.rs`
- Modify: `crates/codex-plus-core/src/lumio/mod.rs`（加 `pub mod launch;`）
- Test: 同文件内 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `crate::app_paths::{resolve_codex_app_dir, codex_app_version, normalize_codex_app_path, build_codex_executable}`
- Produces：
  - `pub fn build_launch_command(app_dir: &Path) -> Result<(String, Vec<String>), String>`
  - `pub fn launch_official_codex(app_dir: &Path) -> Result<(), String>`
  - `pub fn validate_selected_app(path: &Path) -> Result<PathBuf, String>`（失败为 `CODEX_APP_INVALID`）
  - `pub fn open_in_browser(url: &str) -> Result<(), String>`

**关键约束**：启动**不得**传 `--remote-debugging-port`、不得启用注入、不得启动任何 watchdog。这与 `launcher::launch_and_inject` 是两条完全独立的路径，不得复用后者。

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_launches_the_bundle_through_open_without_debugging_flags() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Codex.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();

        let (program, args) = build_launch_command(&app).unwrap();

        assert_eq!(program, "open");
        assert_eq!(args, vec!["-a".to_string(), app.to_string_lossy().into_owned()]);
        assert!(!args.iter().any(|arg| arg.contains("remote-debugging-port")));
        assert!(!args.iter().any(|arg| arg.contains("remote-allow-origins")));
    }

    #[test]
    fn the_launch_command_never_carries_injection_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Codex.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();

        if let Ok((_, args)) = build_launch_command(&app) {
            for arg in &args {
                assert!(!arg.contains("remote-debugging"), "injection flag leaked: {arg}");
                assert!(!arg.contains("inspect"), "injection flag leaked: {arg}");
            }
        }
    }

    #[test]
    fn a_nonexistent_path_is_rejected_as_an_invalid_app() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("Nope.app");

        assert_eq!(validate_selected_app(&missing).unwrap_err(), "CODEX_APP_INVALID");
    }

    #[test]
    fn a_plain_directory_is_rejected_as_an_invalid_app() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("just-a-folder");
        std::fs::create_dir_all(&plain).unwrap();

        assert_eq!(validate_selected_app(&plain).unwrap_err(), "CODEX_APP_INVALID");
    }

    #[test]
    fn only_http_and_https_urls_may_be_opened() {
        for rejected in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,<script>",
            "",
        ] {
            assert_eq!(open_in_browser(rejected).unwrap_err(), "CODEX_APP_INVALID", "{rejected}");
        }
    }
}
```

> 最后一个测试用 `CODEX_APP_INVALID` 作为「拒绝打开」的稳定码是刻意的：本期不引入新的错误域，`open_in_browser` 只在校验失败时返回它，实现里不得为此新增未登记的错误码。

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p codex-plus-core lumio::launch`
Expected: 编译失败 — `cannot find function build_launch_command`

- [ ] **Step 3: Write minimal implementation**

- macOS：`("open", vec!["-a", app_dir])`。
- Windows / 其他：用 `app_paths::build_codex_executable(app_dir)` 拿到可执行文件，`(exe_path, vec![])`；拿不到则 `Err("CODEX_APP_INVALID")`。
- `launch_official_codex`：`std::process::Command::new(program).args(args).spawn()`，失败 `Err("CODEX_LAUNCH_FAILED")`。Windows 上加 `CREATE_NO_WINDOW`（照 `launcher.rs` 现有写法）。
- `validate_selected_app`：转交 `app_paths::normalize_codex_app_path`，`None` → `Err("CODEX_APP_INVALID")`。
- `open_in_browser`：先校验 `url.starts_with("https://") || url.starts_with("http://")`，否则 `Err("CODEX_APP_INVALID")`；macOS `open <url>`、Windows `cmd /C start "" <url>`、其他 `xdg-open <url>`。**不引入** opener 插件。

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p codex-plus-core lumio::launch && cargo fmt --all -- --check`
Expected: 5 tests pass（macOS）

- [ ] **Step 5: Commit**

```bash
git add crates/codex-plus-core/src/lumio/launch.rs crates/codex-plus-core/src/lumio/mod.rs
git commit -m "feat(lumio): launch the official app without injection and guard browser urls"
```

---

### Task 10: Tauri 命令层与白名单

**Files:**
- Modify: `apps/codex-plus-manager/src-tauri/src/lumio_commands.rs`
- Modify: `apps/codex-plus-manager/src-tauri/src/lib.rs:43-47`
- Test: `apps/codex-plus-manager/src-tauri/tests/lumio_command_surface.rs`

**Interfaces:**
- Consumes: Task 4–9 的全部 core 模块
- Produces（前端在 Task 11 绑定的命令名与返回 payload）：

| 命令 | 参数 | payload |
|------|------|---------|
| `lumio_bootstrap` | — | `LumioBootstrapPayload`（新增 `credential_status: CredentialStatus`） |
| `lumio_public_settings` | — | `LumioServiceSettingsPayload` |
| `lumio_send_verify_code` | `email: String` | `{ countdown: u32 }` |
| `lumio_register` | `email, password, verify_code, accepted_revision` | `LumioAuthPayload` |
| `lumio_login` | `email, password` | `LumioAuthPayload` |
| `lumio_login_two_factor` | `code: String` | `LumioAuthPayload` |
| `lumio_logout` | — | `{}` |
| `lumio_refresh_account` | — | `LumioAccountPayload` |
| `lumio_provision_step` | `step: String` | `{ step: String }` |
| `lumio_takeover_health` | — | `{ health: String, error_code: Option<String> }` |
| `lumio_restore_config` | — | `{}` |
| `lumio_launch_codex` | — | `{}` |
| `lumio_select_codex_app` | `path: String` | `LumioCodexAppPayload` |
| `lumio_detect_codex_app` | — | `Option<LumioCodexAppPayload>` |
| `lumio_open_browser` | `url: String` | `{}` |
| `lumio_set_telemetry` | `enabled: bool` | `{ enabled: bool }` |
| `lumio_export_logs` | — | `{ path: String }` |

`LumioAuthPayload = { requires_two_factor: bool, masked_email: Option<String>, account: Option<LumioAccountPayload> }` —— **不含任何 token**。临时 2FA token 保存在后端 `tauri::State` 的进程内 `Mutex<Option<String>>` 中，`lumio_login_two_factor` 从那里取。

- [ ] **Step 1: Write the failing test**

改写 `apps/codex-plus-manager/src-tauri/tests/lumio_command_surface.rs` 的第一个测试并新增两个：

```rust
#[test]
fn lumio_builder_registers_only_the_lumio_allowlist() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(root.join("src/lib.rs")).expect("lib.rs");
    let handler = source
        .split_once(".invoke_handler(tauri::generate_handler![")
        .and_then(|(_, rest)| rest.split_once("])"))
        .map(|(handler, _)| handler)
        .expect("Lumio invoke handler");
    let commands = handler
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.trim_end_matches(','))
        .collect::<Vec<_>>();

    assert_eq!(
        commands,
        [
            "lumio_commands::lumio_bootstrap",
            "lumio_commands::lumio_public_settings",
            "lumio_commands::lumio_send_verify_code",
            "lumio_commands::lumio_register",
            "lumio_commands::lumio_login",
            "lumio_commands::lumio_login_two_factor",
            "lumio_commands::lumio_logout",
            "lumio_commands::lumio_refresh_account",
            "lumio_commands::lumio_provision_step",
            "lumio_commands::lumio_takeover_health",
            "lumio_commands::lumio_restore_config",
            "lumio_commands::lumio_launch_codex",
            "lumio_commands::lumio_detect_codex_app",
            "lumio_commands::lumio_select_codex_app",
            "lumio_commands::lumio_open_browser",
            "lumio_commands::lumio_set_telemetry",
            "lumio_commands::lumio_export_logs",
            "lumio_hide_to_tray",
            "lumio_exit_app",
        ]
    );
}

#[test]
fn every_registered_command_uses_the_lumio_prefix() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(root.join("src/lib.rs")).expect("lib.rs");
    let handler = source
        .split_once(".invoke_handler(tauri::generate_handler![")
        .and_then(|(_, rest)| rest.split_once("])"))
        .map(|(handler, _)| handler)
        .expect("Lumio invoke handler");

    for line in handler.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let command = line.trim_end_matches(',').rsplit("::").next().unwrap();
        assert!(
            command.starts_with("lumio_"),
            "command outside the lumio surface: {command}"
        );
    }
}

#[test]
fn command_payloads_never_expose_tokens_or_key_material() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(root.join("src/lumio_commands.rs")).expect("lumio_commands.rs");

    let payload_structs = source
        .split("#[derive(Debug, Serialize)]")
        .skip(1)
        .collect::<Vec<_>>();
    assert!(!payload_structs.is_empty(), "no serializable payloads found");

    for block in payload_structs {
        let body = block.split_once('}').map(|(head, _)| head).unwrap_or(block);
        for forbidden in ["access_token", "refresh_token", "temp_token", "api_key"] {
            assert!(
                !body.contains(forbidden),
                "serialized payload leaks {forbidden}:\n{body}"
            );
        }
    }
}
```

保留文件中原有的 `lumio_builder_does_not_register_codex_plus_enhancement_commands` 与 `lumio_entrypoint_has_no_legacy_url_or_skin_processing` 两个测试不动。

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p codex-plus-manager --test lumio_command_surface`
Expected: FAIL — `assertion left == right failed`（当前只注册 3 个命令）

- [ ] **Step 3: Write minimal implementation**

在 `lumio_commands.rs`：

- 定义 `pub struct LumioSession { client: LumioApiClient, store: CredentialStore, pending_two_factor: Mutex<Option<String>>, tokens: Mutex<Option<TokenPair>> }`，在 `lib.rs` 的 `setup` 里 `app.manage(LumioSession::new()?)`。
- 每个命令签名形如 `#[tauri::command] pub async fn lumio_login(session: tauri::State<'_, LumioSession>, email: String, password: String) -> Result<LumioCommandResult<LumioAuthPayload>, ()>`。沿用现有 `LumioCommandResult { ok, error_code, payload }` 形状；失败时 `ok: false` + `error_code` 为 Task 4 的稳定码。
- `lumio_provision_step` 按 `step` 分派：`verify-account` → `me()` 并存 account；`prepare-connection` → `ensure_desktop_key()` 并写 `CredentialStore`；`sync-models` → `models()`；`write-config` → `config_takeover::apply_takeover()`。
- 所有进入日志或错误路径的字符串都先过 `lumio::errors::redact`。
- `lib.rs` 的 `invoke_handler` 按测试断言的顺序逐行列出（每行一个命令，末尾逗号），保持解析格式不变。

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p codex-plus-manager && cargo fmt --all -- --check`
Expected: 5 tests pass

- [ ] **Step 5: Commit**

```bash
git add apps/codex-plus-manager/src-tauri/src/lumio_commands.rs apps/codex-plus-manager/src-tauri/src/lib.rs apps/codex-plus-manager/src-tauri/tests/lumio_command_surface.rs
git commit -m "feat(lumio): expose the account command surface behind the lumio allowlist"
```

---

### Task 11: 前端 invoke 绑定与文案清单

**Files:**
- Modify: `apps/codex-plus-manager/src/lumio/invoke.ts`
- Modify: `apps/codex-plus-manager/src/lumio/types.ts`
- Test: `apps/codex-plus-manager/src/lumio/shell-copy.test.ts`

**Interfaces:**
- Consumes: Task 10 的命令名与 payload 形状
- Produces: 每个命令一个 async 包装函数（`loadLumioBootstrap`、`loadPublicSettings`、`sendVerifyCode`、`registerAccount`、`signIn`、`submitTwoFactor`、`signOut`、`refreshAccount`、`runProvisioningStep`、`checkTakeover`、`restoreConfig`、`launchCodex`、`detectCodexApp`、`selectCodexApp`、`openInBrowser`、`setTelemetry`、`exportLogs`），失败时 `throw new LumioCommandError(errorCode)`；`shellLabels` 扩充为覆盖全部视图的可见文案清单。

- [ ] **Step 1: Write the failing test**

改写 `shell-copy.test.ts`：保留「React entry renders only LumioApp」与禁词测试不变，把命令常量断言与标签清单断言换成：

```ts
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

test("a failed command surfaces its stable error code", async () => {
  const error = new LumioCommandError("AUTH_INVALID_CREDENTIALS");
  assert.equal(error.errorCode, "AUTH_INVALID_CREDENTIALS");
  assert.equal(error.message, "AUTH_INVALID_CREDENTIALS");
  assert.ok(error instanceof Error);
});
```

并保留原有禁词测试（它会自动覆盖新加的 11 个标签）。

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/codex-plus-manager && node --test src/lumio/shell-copy.test.ts`
Expected: FAIL — `LUMIO_COMMANDS` 未导出

- [ ] **Step 3: Write minimal implementation**

重写 `invoke.ts`：导出 `LUMIO_COMMANDS as const`、`LumioCommandError extends Error`、扩充后的 `shellLabels` 与 `visibleShellLabels`，以及一个内部 `runCommand<T>(command: string, args?: Record<string, unknown>): Promise<T>` 负责解包 `CommandResult` 并在 `!ok` 时 `throw new LumioCommandError(errorCode ?? "UNKNOWN")`。保留 `LUMIO_BOOTSTRAP_COMMAND` 导出以免破坏既有引用（值改为 `LUMIO_COMMANDS.bootstrap`）。

- [ ] **Step 4: Run test to verify it passes**

Run: `cd apps/codex-plus-manager && npm test && npm run check`
Expected: 全部通过

- [ ] **Step 5: Commit**

```bash
git add apps/codex-plus-manager/src/lumio/invoke.ts apps/codex-plus-manager/src/lumio/types.ts apps/codex-plus-manager/src/lumio/shell-copy.test.ts
git commit -m "feat(lumio): bind the full command surface from the shell"
```

---

### Task 12: 壳层重构与未登录首页

**Files:**
- Modify: `apps/codex-plus-manager/src/LumioApp.tsx`
- Create: `apps/codex-plus-manager/src/lumio/views/SignedOutView.tsx`
- Create: `apps/codex-plus-manager/src/lumio/views/Toast.tsx`
- Modify: `apps/codex-plus-manager/src/lumio-shell.css`

**Interfaces:**
- Consumes: Task 1/2/11
- Produces: `LumioApp` 按 `state.phase` + `state.authStep` 路由到各视图；`useToasts()` hook 返回 `{ toasts, pushToast(code | text), dismiss(id) }`

- [ ] **Step 1: Write the failing test**

React 组件在本仓库无法运行时单测（无 jsdom），因此本任务的自动化门槛是**源码级断言**。在 `shell-copy.test.ts` 追加：

```ts
test("the shell no longer renders marketing-style brand decoration", async () => {
  const shell = await readFile(new URL("../LumioApp.tsx", import.meta.url), "utf8");
  const signedOut = await readFile(
    new URL("./views/SignedOutView.tsx", import.meta.url),
    "utf8",
  );

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
  const signedOut = await readFile(
    new URL("./views/SignedOutView.tsx", import.meta.url),
    "utf8",
  );

  assert.match(signedOut, /更快开始使用官方 Codex。/);
  assert.match(
    signedOut,
    /这个小工具只做一件事：帮你完成注册、登录和本机配置，省去手动安装配置的步骤。之后你使用的始终是官方 Codex 应用，一切保持原生。/,
  );
  assert.match(signedOut, /不修改官方应用/);
  assert.match(signedOut, /配置可一键恢复/);
  assert.match(signedOut, /凭据由系统保护/);
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/codex-plus-manager && node --test src/lumio/shell-copy.test.ts`
Expected: FAIL — `SignedOutView.tsx` 不存在

- [ ] **Step 3: Write minimal implementation**

- `LumioApp.tsx` 收缩为：顶栏（品牌 + 主导航 + 阶段灯）、`<main>` 内按阶段路由、底栏、`<ToastHost/>`。阶段页（register / login / provisioning / repair）渲染时**隐藏主导航的切换按钮**（交互文档 §4）。
- 删除两个 `lumio-aurora` div 与整个 `lumio-orbit-card` 块；同步删除 `lumio-shell.css` 中 `lumio-aurora*` / `lumio-orbit*` 的所有规则与相关 `@keyframes`。
- `SignedOutView`：左侧 Hero（标题「更快开始使用官方 Codex。」+ 定位段落 + 「登录」primary / 「创建账户」secondary 两枚按钮，禁用时在下方渲染 `actionNotes`），右侧「我们的承诺」清单卡三条，底部三格状态条（账户状态 / 官方应用 / 服务入口）。
- 服务不可达时按 §5.1 每 30s 重试 `loadPublicSettings()`，成功则 dispatch `service-settings-loaded` 解禁按钮。
- `Toast.tsx`：右上角、4s 自动消失、最多保留 3 条、错误 toast 用 `lumioErrorLabel` 带错误码后缀。

- [ ] **Step 4: Run test to verify it passes**

Run: `cd apps/codex-plus-manager && npm test && npm run check && npm run vite:build`
Expected: 全部通过，构建成功

- [ ] **Step 5: Commit**

```bash
git add apps/codex-plus-manager/src/LumioApp.tsx apps/codex-plus-manager/src/lumio/views apps/codex-plus-manager/src/lumio-shell.css apps/codex-plus-manager/src/lumio/shell-copy.test.ts
git commit -m "feat(lumio): restrain the shell visuals and implement the signed-out surface"
```

---

### Task 13: 注册页与登录页（含 2FA）

**Files:**
- Create: `apps/codex-plus-manager/src/lumio/views/RegisterView.tsx`
- Create: `apps/codex-plus-manager/src/lumio/views/LoginView.tsx`
- Modify: `apps/codex-plus-manager/src/LumioApp.tsx`（接线）
- Modify: `apps/codex-plus-manager/src/lumio-shell.css`
- Test: `apps/codex-plus-manager/src/lumio/shell-copy.test.ts`（源码级断言）

**Interfaces:**
- Consumes: Task 3 的 `registerFormError` / `sanitizeVerifyCode` / `passwordStrength` / `formatEmailSuffixHint`；Task 11 的 `sendVerifyCode` / `registerAccount` / `signIn` / `submitTwoFactor` / `openInBrowser`
- Produces: 两个视图组件，props 为 `{ settings, onAuthenticated, onTwoFactorRequired, onBack, pushToast }`

- [ ] **Step 1: Write the failing test**

追加到 `shell-copy.test.ts`：

```ts
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/codex-plus-manager && node --test src/lumio/shell-copy.test.ts`
Expected: FAIL — `RegisterView.tsx` 不存在

- [ ] **Step 3: Write minimal implementation**

**RegisterView**（交互文档 5.2）：居中窄卡 `max-width: 420px`；字段顺序 标题 / 邮箱 / 验证码行 / 密码 / 确认密码 / 协议勾选组 / 提交 / 底部链接。

- 邮箱失焦校验；后缀提示常驻，用 `formatEmailSuffixHint(settings.emailSuffixWhitelist)`。
- 发送验证码按钮：仅 `isValidEmail` 为真时可点；点击后 `useEffect` 驱动 60s 倒计时，按钮文案 `重新发送 (${remaining}s)`；失败恢复并 `pushToast(errorCode)`。
- 验证码输入 `onChange` 一律过 `sanitizeVerifyCode`。
- 密码强度条读 `passwordStrength`；确认密码失焦比对。
- 协议勾选组遍历 `settings.agreementDocuments`，标题可点击展开 `contentMd`（纯文本渲染即可，不引入 markdown 库），全部勾选前提交禁用。
- 提交前调 `registerFormError`。它返回的可能是**客户端字段级标识**（不在 `LUMIO_ERROR_COPY` 里，交给 `lumioErrorCopy` 只会得到笼统的兜底文案），因此视图内必须有一张字段级标识 → 文案的映射：`EMAIL_FORMAT_INVALID` → 「请输入有效的邮箱地址」，`PASSWORD_TOO_SHORT` → 「密码至少 8 位」，`PASSWORD_MISMATCH` → 「两次输入不一致」，`AGREEMENTS_NOT_ACCEPTED` → 「请先阅读并勾选全部协议」。只有不在这张表里的码才回落到 `lumioErrorCopy`。
- 提交中禁用全部输入；服务端失败在卡片顶部渲染错误横幅（`lumioErrorLabel`），**保留已填内容**。
- `settings.registrationEnabled === false` 时整卡替换为说明面板：标题「注册暂未开放」+ 「返回登录」按钮，不渲染任何表单控件。

**LoginView**（交互文档 5.3）：同构窄卡。

- 凭据错误：顶部横幅 `lumioErrorCopy("AUTH_INVALID_CREDENTIALS")`，清空密码、保留邮箱、焦点回密码框。
- 2FA：卡片**原位**切到第二步，6 个 `<input maxLength={1}>` 分格，自动前进 / 退格回退 / `onPaste` 分发；错误时清空输入并加一次抖动 class。
- 忘记密码：弹说明层「密码重置在网页端完成」+ 「在浏览器中打开」按钮 → `openInBrowser(`${settings.siteBaseUrl}/reset-password`)`。
- 账号停用：横幅用 `lumioErrorLabel("AUTH_ACCOUNT_DISABLED")` + 「联系支持」链接。

- [ ] **Step 4: Run test to verify it passes**

Run: `cd apps/codex-plus-manager && npm test && npm run check && npm run vite:build`
Expected: 全部通过

- [ ] **Step 5: Commit**

```bash
git add apps/codex-plus-manager/src/lumio/views/RegisterView.tsx apps/codex-plus-manager/src/lumio/views/LoginView.tsx apps/codex-plus-manager/src/LumioApp.tsx apps/codex-plus-manager/src/lumio-shell.css apps/codex-plus-manager/src/lumio/shell-copy.test.ts
git commit -m "feat(lumio): implement the registration and login surfaces"
```

---

### Task 14: Provisioning 过渡页与首页在线 / 离线

**Files:**
- Create: `apps/codex-plus-manager/src/lumio/views/ProvisioningView.tsx`
- Create: `apps/codex-plus-manager/src/lumio/views/HomeView.tsx`
- Modify: `apps/codex-plus-manager/src/LumioApp.tsx`
- Modify: `apps/codex-plus-manager/src/lumio-shell.css`
- Test: `apps/codex-plus-manager/src/lumio/shell-copy.test.ts`

**Interfaces:**
- Consumes: Task 2 的 `PROVISIONING_STEP_IDS` / `PROVISIONING_STEP_TITLES` / `LumioProvisioning` / `actionNotes`；Task 11 的 `runProvisioningStep` / `refreshAccount` / `launchCodex`
- Produces: 两个视图组件

- [ ] **Step 1: Write the failing test**

```ts
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/codex-plus-manager && node --test src/lumio/shell-copy.test.ts`
Expected: FAIL — 视图文件不存在

- [ ] **Step 3: Write minimal implementation**

**ProvisioningView**（5.4）：居中垂直步骤列表，遍历 `PROVISIONING_STEP_IDS` 渲染图标 + `PROVISIONING_STEP_TITLES[id]` + 状态（等待 / spinner / ✓ / ✕）。

- 由父组件驱动顺序执行：`for` 循环 `await runProvisioningStep(id)`，每步前 dispatch `provisioning-step-started`，成功 dispatch `provisioning-step-completed`，抛错 dispatch `provisioning-step-failed`。
- 单步计时超 10s 显示「比平时慢一些，仍在继续…」。
- 全部完成后 `setTimeout(..., 600)` 再 dispatch `online-ready`。
- 失败时渲染 `lumioErrorLabel(provisioning.errorCode)` + 「重试」（从 `failedStep` 续跑）与「稍后处理」（`signOut()` 回未登录首页）；`provisioning.suggestRepair` 为真时额外提示进入修复页。
- 页面**不渲染任何返回按钮**。

**HomeView**（5.5）：欢迎行（邮箱 + 「凭据由系统保护」徽章）→ 三张指标卡 → 行动面板。

- 余额卡：`formatBalance(account.balance)` + 套餐摘要；离线时数值降灰并加「缓存值」标注。
- 连接状态卡：在线显示「在线」+ `cachedAt` 格式化后的最近同步时间 + 「刷新」小按钮（≤3s spinner，失败 `pushToast("刷新失败，仍显示上次数据")`）；离线时刷新按钮禁用并挂 tooltip `actionNotes.refresh`。
- 默认模型卡：`state.defaultModel` + 「已配置」徽章，无本地切换入口。
- 行动面板：「充值」按钮恒禁用并渲染 `actionNotes.pay`（本期范围外，不伪装可用）；「启动 Codex」按钮 `disabled={!actions.canLaunch}`，禁用时渲染 `actionNotes.launch` 并链接设置页；点击成功 `pushToast("官方 Codex 已启动")`，失败 `pushToast("CODEX_LAUNCH_FAILED")`。
- 离线态：欢迎行下方常驻信息条「无法连接服务，正在使用 <时间> 的本机缓存。你仍可以启动官方 Codex。」；后台探测恢复后信息条变绿「已重新连接」，3s 后消失并刷新数据。

- [ ] **Step 4: Run test to verify it passes**

Run: `cd apps/codex-plus-manager && npm test && npm run check && npm run vite:build`
Expected: 全部通过

- [ ] **Step 5: Commit**

```bash
git add apps/codex-plus-manager/src/lumio/views/ProvisioningView.tsx apps/codex-plus-manager/src/lumio/views/HomeView.tsx apps/codex-plus-manager/src/LumioApp.tsx apps/codex-plus-manager/src/lumio-shell.css apps/codex-plus-manager/src/lumio/shell-copy.test.ts
git commit -m "feat(lumio): implement provisioning progress and the online/offline home"
```

---

### Task 15: 修复页与设置页

**Files:**
- Create: `apps/codex-plus-manager/src/lumio/views/RepairView.tsx`
- Create: `apps/codex-plus-manager/src/lumio/views/SettingsView.tsx`（从 `LumioApp.tsx` 迁出并补齐交互）
- Modify: `apps/codex-plus-manager/src/LumioApp.tsx`
- Modify: `apps/codex-plus-manager/src/lumio-shell.css`
- Test: `apps/codex-plus-manager/src/lumio/shell-copy.test.ts`

**Interfaces:**
- Consumes: Task 11 的 `checkTakeover` / `restoreConfig` / `detectCodexApp` / `selectCodexApp` / `setTelemetry` / `exportLogs`
- Produces: 两个视图组件

- [ ] **Step 1: Write the failing test**

```ts
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/codex-plus-manager && node --test src/lumio/shell-copy.test.ts`
Expected: FAIL — 视图文件不存在

- [ ] **Step 3: Write minimal implementation**

**RepairView**（5.7）：警示图标 + 标题「需要检查配置」+ 依错误码分支的说明 + 错误码 chip + 三个动作。

- 说明分支：`CODEX_CONFIG_CONFLICT` → 「检测到本机配置被其他工具修改过」；`AUTH_SESSION_EXPIRED` / `KEY_STORAGE_UNAVAILABLE` → 「本机保存的登录凭据已失效，请重新登录」，且此时动作 1 替换为「重新登录」。
- 动作 2「恢复本机配置」为警示色，点击弹二次确认，文案一字不差用上面测试断言的那句。
- 动作 3「导出诊断日志」，说明「导出前会再次扫描并移除敏感内容」。
- 失败时显示「问题仍未解决」+ 重试，**不提供任何强制覆盖按钮**。

**SettingsView**（5.8）：六行保持原样，补齐交互。

- 开机启动 / 自动更新：即点即生效，失败回弹 + toast；自动更新关闭时下方出现灰字「你仍会收到重要安全更新提示」。
- 官方应用路径：「重新检测」按钮 spinner；成功时路径行加一次绿色闪烁 class；失败保留原值 + 行内「未检测到，可手动选择」并出现「手动选择…」按钮，用已注册的 `tauri-plugin-dialog` 打开文件选择器（capability 已含 `dialog:default`，无需改配置）；校验失败 toast `lumioErrorLabel("CODEX_APP_INVALID")`。
- 遥测：默认关；开启时弹说明层列出四类字段（版本 / 平台 / 阶段 / 脱敏错误码），确认后才调 `setTelemetry(true)`。
- 日志导出：loading → 完成 toast「已导出到 <路径>」。
- 配置恢复：与修复页动作 2 完全一致的二次确认与结果反馈。
- 页尾常驻「不可用的选项会保持禁用，不会修改本机配置。」

**开机启动**本期无后端命令支撑（不在 Task 10 的命令表内），因此该行保持禁用并在其说明位注明「本机开机启动尚未开放」——按硬约束「涉及处保持禁用态 + 状态说明，不装成功」。

- [ ] **Step 4: Run test to verify it passes**

Run: `cd apps/codex-plus-manager && npm test && npm run check && npm run vite:build`
Expected: 全部通过

- [ ] **Step 5: Commit**

```bash
git add apps/codex-plus-manager/src/lumio/views/RepairView.tsx apps/codex-plus-manager/src/lumio/views/SettingsView.tsx apps/codex-plus-manager/src/LumioApp.tsx apps/codex-plus-manager/src/lumio-shell.css apps/codex-plus-manager/src/lumio/shell-copy.test.ts
git commit -m "feat(lumio): implement the repair and settings surfaces"
```

---

### Task 16: 收口验证与禁词全量扫描

**Files:**
- Test: `apps/codex-plus-manager/src/lumio/shell-copy.test.ts`（新增全源码扫描）

**Interfaces:**
- Consumes: 全部前序任务
- Produces: 一个覆盖整个 `src/lumio/views/` 与 `LumioApp.tsx` 的禁词扫描测试

- [ ] **Step 1: Write the failing test**

```ts
test("no user-facing source file mentions a forbidden product surface", async () => {
  const { readdir } = await import("node:fs/promises");
  const viewsDir = new URL("./views/", import.meta.url);
  const files = await readdir(viewsDir);
  const sources = await Promise.all([
    readFile(new URL("../LumioApp.tsx", import.meta.url), "utf8"),
    ...files
      .filter((name) => name.endsWith(".tsx"))
      .map((name) => readFile(new URL(name, viewsDir), "utf8")),
  ]);

  assert.ok(files.length >= 7, `expected every view to exist, found: ${files.join(", ")}`);

  for (const source of sources) {
    const lowered = source.toLowerCase();
    for (const forbidden of ["base url", "stepwise", "dream skin", "api key", "mcp", "plugin"]) {
      assert.equal(lowered.includes(forbidden), false, `forbidden term "${forbidden}" in a view`);
    }
  }
});
```

> 说明：`provider` 一词不进本条扫描的字面列表，因为 React 的 `Context.Provider` 是框架 API 而非产品用语；若实现中确实用到 Context，评审时需确认它不出现在任何面向用户的字符串里。`shellLabels` 那条既有测试仍然对 `provider` 保持红线。

- [ ] **Step 2: Run test to verify it fails**

在实现前先临时把某个视图里加一处 `api key` 字样跑一次，确认测试能抓到，然后撤销。
Run: `cd apps/codex-plus-manager && node --test src/lumio/shell-copy.test.ts`
Expected: FAIL，且失败信息指出具体禁词

- [ ] **Step 3: 修正所有命中**

按扫描结果逐一改写文案。

- [ ] **Step 4: 跑完整收口门槛**

```bash
cargo fmt --all -- --check
cargo test -p codex-plus-manager -p codex-plus-core
cd apps/codex-plus-manager && npm run check && npm test && npm run vite:build
```

Expected: 四条命令全绿。另外手动执行禁词扫描并留档：

```bash
rg -in "base url|stepwise|dream skin|api key" apps/codex-plus-manager/src --glob '!*.test.ts'
```

Expected: 无输出。

- [ ] **Step 5: Commit**

```bash
git add apps/codex-plus-manager/src/lumio/shell-copy.test.ts
git commit -m "test(lumio): scan every user-facing view for forbidden product surfaces"
```

---

## 已知缺口（交付时必须如实声明）

1. **凭据未落系统凭据库**：按 ADR `0001` 存本地受限权限文件，与设计文档 8.1 冲突。
2. **支付交接未实现**：「充值」按钮恒禁用并注明原因；`PAYMENT_HANDOFF_*` 错误码已登记但无调用方。
3. **自动更新未实现**：设置项存在但不触发任何更新逻辑；`UPDATE_VERIFY_FAILED` 同上。
4. **遥测不真实发送**：开关与确认层可用，但没有上报通道。
5. **`GET /api/v1/desktop/config` 服务端不存在**：默认模型改从 `settings/public` 的 `ccswitch_default_model_openai` 读取，`minimum_client_version` 与 `payment_path` 本期无来源，故 `SERVICE_VERSION_TOO_OLD` 无触发点。
6. **开机启动无后端命令**：设置行禁用并注明。
7. **React 组件无运行时测试**：仓库无 jsdom / 测试运行时，视图层只有源码级断言 + `tsc` + 构建覆盖，交互行为靠原型对照人工验收。
8. **官网不在本期范围**。
