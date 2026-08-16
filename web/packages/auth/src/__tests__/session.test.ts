import { afterEach, describe, expect, it } from "vitest";

import {
  clearSession,
  hasSession,
  readRefreshToken,
  readSession,
  serializeCookie,
  writeSession,
} from "../session";

afterEach(() => {
  clearSession();
});

describe("serializeCookie", () => {
  it("生产域下写父域 Cookie 并带 Secure，三站共享同一会话", () => {
    const cookie = serializeCookie("lumio_at", "abc", {
      maxAge: 3600,
      hostname: "cc.bestcodex.app",
      secure: true,
    });

    expect(cookie).toContain("lumio_at=abc");
    expect(cookie).toContain("Domain=.bestcodex.app");
    expect(cookie).toContain("Path=/");
    expect(cookie).toContain("SameSite=Lax");
    expect(cookie).toContain("Max-Age=3600");
    expect(cookie).toContain("Secure");
  });

  it("开发环境（localhost / http）不写 Domain、不写 Secure", () => {
    const cookie = serializeCookie("lumio_at", "abc", {
      maxAge: 3600,
      hostname: "localhost",
      secure: false,
    });

    expect(cookie).not.toContain("Domain=");
    expect(cookie).not.toContain("Secure");
  });

  it("值做 URL 编码，令牌里的特殊字符不会截断 Cookie", () => {
    expect(serializeCookie("lumio_rt", "a;b c", { maxAge: 1, hostname: "localhost" })).toContain(
      "lumio_rt=a%3Bb%20c",
    );
  });
});

describe("会话读写", () => {
  it("写入后能读回令牌与过期时间", () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });

    const session = readSession();

    expect(session?.accessToken).toBe("at-1");
    expect(session?.refreshToken).toBe("rt-1");
    expect(session?.expiresAt).toBeGreaterThan(Date.now());
  });

  it("没有令牌时返回 null，子站据此判定未登录", () => {
    expect(readSession()).toBeNull();
  });

  it("登出后 Cookie 被清掉", () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    clearSession();

    expect(readSession()).toBeNull();
  });
});

describe("可续期会话的判定（W-1）", () => {
  it("access cookie 到期被浏览器删除后，refresh cookie 仍可读回", () => {
    document.cookie = serializeCookie("lumio_rt", "rt-1", { maxAge: 3600, hostname: "localhost" });

    expect(readSession()).toBeNull();
    expect(readRefreshToken()).toBe("rt-1");
    expect(hasSession()).toBe(true);
  });

  it("两种 cookie 都不在时才认定没有会话", () => {
    expect(readRefreshToken()).toBeNull();
    expect(hasSession()).toBe(false);
  });

  it("只有 access 也在时同样视为有会话", () => {
    document.cookie = serializeCookie("lumio_at", "at-1", { maxAge: 3600, hostname: "localhost" });

    expect(hasSession()).toBe(true);
  });
});
