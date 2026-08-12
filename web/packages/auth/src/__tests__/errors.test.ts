import { describe, expect, it } from "vitest";

import { errorText, normalizeReason } from "../errors";

/**
 * 映射表与 codex/crates/codex-plus-core/src/lumio/errors.rs 保持同一口径：
 * 桌面端与官网对同一个服务端 reason 必须给出同一个稳定码。
 */
describe("normalizeReason", () => {
  it("凭据类原因收敛成同一个码，避免账号枚举", () => {
    expect(normalizeReason(401, "INVALID_CREDENTIALS")).toBe("AUTH_INVALID_CREDENTIALS");
    expect(normalizeReason(401, "INVALID_USER")).toBe("AUTH_INVALID_CREDENTIALS");
  });

  it("停用账号与密码错误可区分", () => {
    expect(normalizeReason(403, "USER_NOT_ACTIVE")).toBe("AUTH_ACCOUNT_DISABLED");
  });

  it("验证码类原因各自映射", () => {
    expect(normalizeReason(400, "INVALID_VERIFY_CODE")).toBe("AUTH_CODE_INVALID");
    expect(normalizeReason(429, "VERIFY_CODE_MAX_ATTEMPTS")).toBe("AUTH_CODE_INVALID");
    expect(normalizeReason(429, "VERIFY_CODE_TOO_FREQUENT")).toBe("AUTH_CODE_RATE_LIMITED");
    expect(normalizeReason(400, "EMAIL_VERIFY_REQUIRED")).toBe("AUTH_CODE_REQUIRED");
    expect(normalizeReason(400, "VERIFY_CODE_REQUIRED")).toBe("AUTH_CODE_REQUIRED");
  });

  it("注册类原因各自映射", () => {
    expect(normalizeReason(403, "REGISTRATION_DISABLED")).toBe("AUTH_REGISTRATION_CLOSED");
    expect(normalizeReason(400, "EMAIL_SUFFIX_NOT_ALLOWED")).toBe("AUTH_EMAIL_DOMAIN_NOT_ALLOWED");
    expect(normalizeReason(400, "EMAIL_RESERVED")).toBe("AUTH_EMAIL_DOMAIN_NOT_ALLOWED");
    expect(normalizeReason(409, "EMAIL_EXISTS")).toBe("AUTH_EMAIL_ALREADY_REGISTERED");
    expect(normalizeReason(400, "INVALID_EMAIL")).toBe("AUTH_EMAIL_INVALID");
  });

  it("邀请码原因保持可操作，不伪装成服务故障", () => {
    expect(normalizeReason(400, "INVITATION_CODE_REQUIRED")).toBe("AUTH_INVITATION_CODE_REQUIRED");
    expect(normalizeReason(400, "INVITATION_CODE_INVALID")).toBe("AUTH_INVITATION_CODE_INVALID");
    expect(normalizeReason(200, "INVITATION_CODE_DISABLED")).toBe("AUTH_INVITATION_CODE_INVALID");
  });

  it("两步验证原因各自映射", () => {
    expect(normalizeReason(400, "TOTP_INVALID_CODE")).toBe("AUTH_2FA_INVALID");
    expect(normalizeReason(429, "TOTP_TOO_MANY_ATTEMPTS")).toBe("AUTH_2FA_INVALID");
    for (const reason of [
      "TOTP_NOT_SETUP",
      "TOTP_NOT_ENABLED",
      "TOTP_SETUP_EXPIRED",
      "TOTP_VERIFY_ERROR",
      "EMAIL_VERIFY_NOT_ENABLED",
    ]) {
      expect(normalizeReason(400, reason)).toBe("AUTH_2FA_UNAVAILABLE");
    }
  });

  it("所有令牌失效原因收敛成一个会话过期码", () => {
    for (const reason of [
      "INVALID_TOKEN",
      "TOKEN_EXPIRED",
      "ACCESS_TOKEN_EXPIRED",
      "TOKEN_REVOKED",
      "TOKEN_TOO_LARGE",
      "REFRESH_TOKEN_INVALID",
      "REFRESH_TOKEN_EXPIRED",
      "REFRESH_TOKEN_REUSED",
      "SESSION_BINDING_MISMATCH",
    ]) {
      expect(normalizeReason(401, reason)).toBe("AUTH_SESSION_EXPIRED");
    }
  });

  it("未识别的原因不泄漏服务端原文", () => {
    expect(normalizeReason(418, "SOME_BRAND_NEW_SERVER_REASON")).toBe("SERVICE_UNAVAILABLE");
    expect(normalizeReason(401, "SOME_UNRECOGNIZED_REASON")).toBe("AUTH_SESSION_EXPIRED");
    expect(normalizeReason(429, null)).toBe("SERVICE_RATE_LIMITED");
    expect(normalizeReason(500, null)).toBe("SERVICE_UNAVAILABLE");
  });
});

describe("errorText", () => {
  it("每个稳定码都有可展示的中文文案", () => {
    expect(errorText("AUTH_INVALID_CREDENTIALS")).toBe("邮箱或密码不正确。");
    expect(errorText("SERVICE_RATE_LIMITED")).toContain("频繁");
    expect(errorText("UNKNOWN_CODE" as never)).toBe(errorText("SERVICE_UNAVAILABLE"));
  });
});
