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

test("registration blockers the server can report all have their own copy", () => {
  assert.equal(LUMIO_ERROR_COPY.AUTH_CODE_REQUIRED, "请先获取邮箱验证码");
  assert.equal(LUMIO_ERROR_COPY.AUTH_EMAIL_ALREADY_REGISTERED, "该邮箱已注册，请直接登录");
  assert.equal(LUMIO_ERROR_COPY.AUTH_EMAIL_INVALID, "请填写有效的邮箱地址");
  assert.equal(LUMIO_ERROR_COPY.AUTH_INVITATION_CODE_REQUIRED, "注册需要邀请码，请填写后重试");
  assert.equal(LUMIO_ERROR_COPY.AUTH_INVITATION_CODE_INVALID, "邀请码无效或已被使用");
  assert.equal(LUMIO_ERROR_COPY.AUTH_2FA_UNAVAILABLE, "两步验证当前不可用，请联系支持");
  assert.equal(LUMIO_ERROR_COPY.SERVICE_RATE_LIMITED, "请求过于频繁，请稍后再试");
  assert.equal(LUMIO_ERROR_COPY.CODEX_CONFIG_WRITE_FAILED, "写入本机配置失败，已保留原始内容");
});

test("codes added beyond the baseline stay inside the approved domains", () => {
  const domains = ["AUTH_", "ACCOUNT_", "KEY_", "SERVICE_", "CODEX_", "PAYMENT_HANDOFF_", "UPDATE_"];
  for (const code of Object.keys(LUMIO_ERROR_COPY)) {
    if (code === "UNKNOWN") continue;
    assert.ok(
      domains.some((domain) => code.startsWith(domain)),
      `error code outside the approved domains: ${code}`,
    );
  }
});

test("gateway account and catalog states get actionable copy instead of outage copy", () => {
  assert.equal(LUMIO_ERROR_COPY.ACCOUNT_INSUFFICIENT_BALANCE, "账户余额不足，请先充值");
  assert.equal(
    LUMIO_ERROR_COPY.SERVICE_MODEL_CATALOG_EMPTY,
    "当前没有可用模型，请稍后重试或联系支持",
  );
  assert.equal(
    lumioErrorLabel("ACCOUNT_INSUFFICIENT_BALANCE"),
    "账户余额不足，请先充值（ACCOUNT_INSUFFICIENT_BALANCE）",
  );
  assert.equal(
    lumioErrorLabel("SERVICE_MODEL_CATALOG_EMPTY"),
    "当前没有可用模型，请稍后重试或联系支持（SERVICE_MODEL_CATALOG_EMPTY）",
  );
});

test("unknown and empty codes fall back without throwing", () => {
  assert.equal(lumioErrorCopy("NOT_A_REAL_CODE"), "出现未知问题，请稍后重试");
  assert.equal(lumioErrorCopy(null), "出现未知问题，请稍后重试");
  assert.equal(lumioErrorCopy(undefined), "出现未知问题，请稍后重试");
});

test("codes that collide with object prototype members still fall back", () => {
  for (const inherited of ["toString", "constructor", "valueOf", "hasOwnProperty"]) {
    assert.equal(lumioErrorCopy(inherited), "出现未知问题，请稍后重试");
    assert.equal(lumioErrorLabel(inherited), "出现未知问题，请稍后重试（UNKNOWN）");
  }
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
