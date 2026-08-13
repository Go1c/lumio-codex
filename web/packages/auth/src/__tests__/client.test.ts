import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  LumioApiError,
  fetchProfile,
  fetchPublicSettings,
  login,
  loginTwoFactor,
  register,
  sendVerifyCode,
} from "../client";

const fetchMock = vi.fn();

function envelope(data: unknown, status = 200) {
  return new Response(JSON.stringify({ code: 0, message: "success", data }), { status });
}

function failure(status: number, reason: string) {
  return new Response(JSON.stringify({ code: status, message: "服务端原文", reason }), { status });
}

beforeEach(() => {
  fetchMock.mockReset();
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("settings/public", () => {
  it("读出注册开关、验证码开关、邮箱白名单与协议", async () => {
    fetchMock.mockResolvedValue(
      envelope({
        registration_enabled: true,
        email_verify_enabled: true,
        registration_email_suffix_whitelist: ["@example.com"],
        password_reset_enabled: true,
        login_agreement_enabled: true,
        login_agreement_revision: "abc123",
        login_agreement_documents: [{ id: "terms", title: "服务条款", content_md: "# 条款" }],
      }),
    );

    const settings = await fetchPublicSettings();

    expect(fetchMock.mock.calls[0][0]).toBe("https://api.lumio.games/api/v1/settings/public");
    expect(settings.registrationEnabled).toBe(true);
    expect(settings.emailVerifyEnabled).toBe(true);
    expect(settings.emailSuffixWhitelist).toEqual(["@example.com"]);
    expect(settings.agreementRevision).toBe("abc123");
    expect(settings.agreementDocuments[0].title).toBe("服务条款");
  });

  it("缺省字段安全回落，不把 undefined 带进 UI", async () => {
    fetchMock.mockResolvedValue(envelope({ registration_enabled: false }));

    const settings = await fetchPublicSettings();

    expect(settings.registrationEnabled).toBe(false);
    expect(settings.emailSuffixWhitelist).toEqual([]);
    expect(settings.agreementDocuments).toEqual([]);
  });
});

describe("登录 / 注册", () => {
  it("成功时返回令牌与账户资料", async () => {
    fetchMock.mockResolvedValue(
      envelope({
        access_token: "header.payload.signature",
        refresh_token: "rt_abc",
        expires_in: 3600,
        user: { id: 7, email: "user@example.com", balance: 12.5, status: "active" },
      }),
    );

    const outcome = await login("user@example.com", "supersecret");

    expect(outcome.kind).toBe("tokens");
    if (outcome.kind !== "tokens") throw new Error("unreachable");
    expect(outcome.tokens.accessToken).toBe("header.payload.signature");
    expect(outcome.tokens.expiresIn).toBe(3600);
    expect(outcome.profile.balance).toBe(12.5);

    const [, init] = fetchMock.mock.calls[0];
    expect(JSON.parse(init.body)).toEqual({ email: "user@example.com", password: "supersecret" });
  });

  it("2FA 挑战同样是 200，靠 requires_2fa 分支而不是状态码", async () => {
    fetchMock.mockResolvedValue(
      envelope({ requires_2fa: true, temp_token: "tmp_123", user_email_masked: "u***@example.com" }),
    );

    const outcome = await login("user@example.com", "supersecret");

    expect(outcome).toEqual({ kind: "2fa", tempToken: "tmp_123", maskedEmail: "u***@example.com" });
  });

  it("2FA 第二步把 temp_token 与 totp_code 一起提交", async () => {
    fetchMock.mockResolvedValue(
      envelope({
        access_token: "at",
        refresh_token: "rt",
        expires_in: 3600,
        user: { id: 7, email: "user@example.com", balance: 0, status: "active" },
      }),
    );

    await loginTwoFactor("tmp_123", "654321");

    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("https://api.lumio.games/api/v1/auth/login/2fa");
    expect(JSON.parse(init.body)).toEqual({ temp_token: "tmp_123", totp_code: "654321" });
  });

  it("注册时省略未填的验证码与邀请码字段", async () => {
    fetchMock.mockResolvedValue(
      envelope({
        access_token: "at",
        refresh_token: "rt",
        expires_in: 60,
        user: { id: 1, email: "a@example.com", balance: 0, status: "active" },
      }),
    );

    await register({ email: "a@example.com", password: "pw12345678" });

    const [, init] = fetchMock.mock.calls[0];
    expect(JSON.parse(init.body)).toEqual({ email: "a@example.com", password: "pw12345678" });
  });

  it("凭据错误抛出稳定错误码而非服务端原文", async () => {
    fetchMock.mockResolvedValue(failure(401, "INVALID_CREDENTIALS"));

    const error = await login("user@example.com", "nope").catch((e) => e);

    expect(error).toBeInstanceOf(LumioApiError);
    expect(error.code).toBe("AUTH_INVALID_CREDENTIALS");
    expect(error.message).not.toContain("服务端原文");
  });

  it("业务失败也可能是 HTTP 200，靠信封 code 判定", async () => {
    fetchMock.mockResolvedValue(
      new Response(JSON.stringify({ code: 400, message: "x", reason: "EMAIL_EXISTS" }), {
        status: 200,
      }),
    );

    const error = await register({ email: "a@example.com", password: "pw" }).catch((e) => e);

    expect(error.code).toBe("AUTH_EMAIL_ALREADY_REGISTERED");
  });
});

describe("异常响应兜底", () => {
  it("限流响应不套信封，同样归一化成稳定码", async () => {
    fetchMock.mockResolvedValue(
      new Response(JSON.stringify({ error: "rate limit exceeded" }), { status: 429 }),
    );

    const error = await sendVerifyCode("user@example.com").catch((e) => e);

    expect(error.code).toBe("SERVICE_RATE_LIMITED");
  });

  it("非 JSON 的成功响应不炸，退回服务不可用", async () => {
    fetchMock.mockResolvedValue(new Response("not json", { status: 200 }));

    const error = await fetchProfile("access-token").catch((e) => e);

    expect(error.code).toBe("SERVICE_UNAVAILABLE");
  });

  it("网络故障归一化成服务不可用", async () => {
    fetchMock.mockRejectedValue(new TypeError("Failed to fetch"));

    const error = await fetchPublicSettings().catch((e) => e);

    expect(error.code).toBe("SERVICE_UNAVAILABLE");
  });
});

describe("鉴权请求", () => {
  it("/auth/me 带 Bearer 头", async () => {
    fetchMock.mockResolvedValue(
      envelope({ id: 7, email: "user@example.com", balance: 3.25, status: "active" }),
    );

    const profile = await fetchProfile("access-token");

    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("https://api.lumio.games/api/v1/auth/me");
    expect(init.headers.Authorization).toBe("Bearer access-token");
    expect(profile.balance).toBe(3.25);
  });

  it("发送验证码返回服务端倒计时", async () => {
    fetchMock.mockResolvedValue(envelope({ countdown: 60 }));

    await expect(sendVerifyCode("user@example.com")).resolves.toBe(60);
  });
});
