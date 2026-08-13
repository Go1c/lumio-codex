import { http } from "msw";
import { describe, expect, it } from "vitest";

import { getMe } from "@/api/endpoints";
import { ApiError, api } from "@/lib/api";
import { db } from "@/mocks/db";
import { fail, ok } from "@/mocks/handlers";
import { server } from "@/mocks/server";

const API = "*/api/v1";

/** 会话续期约定：401 session_expired → POST /auth/refresh → 原请求重试一次。 */
describe("API 客户端", () => {
  it("解包 {\"data\": ...} 信封", async () => {
    db.loggedIn = true;
    const snapshot = await getMe();
    expect(snapshot.user.id).toBe("U-100986");
  });

  it("把 {\"error\":{...}} 转成带 code 与 details 的 ApiError", async () => {
    server.use(
      http.get(`${API}/me/`, () =>
        fail(429, "rate_limited", "尝试次数过多，请 1 分钟后再试。", { retry_after_seconds: 60 }),
      ),
    );

    const error = await getMe().catch((err: unknown) => err);
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).code).toBe("rate_limited");
    expect((error as ApiError).message).toBe("尝试次数过多，请 1 分钟后再试。");
    expect((error as ApiError).retryAfterSeconds).toBe(60);
  });

  it("session_expired 时刷新一次并重试原请求", async () => {
    let attempts = 0;
    let refreshCalls = 0;

    server.use(
      http.get(`${API}/me/`, () => {
        attempts += 1;
        if (attempts === 1) return fail(401, "session_expired", "登录已过期，请重新登录。");
        return ok({ user: db.user, entitlement: db.entitlement });
      }),
      http.post(`${API}/auth/refresh`, () => {
        refreshCalls += 1;
        return ok({ expires_in: 900 });
      }),
    );

    const snapshot = await getMe();
    expect(snapshot.user.email).toBe(db.user.email);
    expect(refreshCalls).toBe(1);
    expect(attempts).toBe(2);
  });

  it("刷新失败时抛出 session_expired，不再无限重试", async () => {
    let attempts = 0;

    server.use(
      http.get(`${API}/me/`, () => {
        attempts += 1;
        return fail(401, "session_expired", "登录已过期，请重新登录。");
      }),
      http.post(`${API}/auth/refresh`, () => fail(401, "session_expired", "登录已过期，请重新登录。")),
    );

    const error = await getMe().catch((err: unknown) => err);
    expect((error as ApiError).code).toBe("session_expired");
    expect((error as ApiError).message).toBe("登录已过期，请重新登录。");
    expect(attempts).toBe(1);
  });

  it("204 响应解析为 undefined", async () => {
    db.loggedIn = true;
    await expect(api.post<void>("/auth/logout")).resolves.toBeUndefined();
  });
});
