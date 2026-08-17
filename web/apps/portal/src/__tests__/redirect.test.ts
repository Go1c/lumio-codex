import { describe, expect, it } from "vitest";

import { denyRedirectUrl, isAllowedDesktopRedirect, redirectTarget } from "@/lib/redirect";

describe("redirectTarget", () => {
  it("站内路径走前端路由，不整页刷新", () => {
    expect(redirectTarget("/account", "/account")).toEqual({ kind: "internal", path: "/account" });
  });

  it("子站地址整页跳回去", () => {
    expect(redirectTarget("https://cc.bestcodex.app/pricing", "/account")).toEqual({
      kind: "external",
      url: "https://cc.bestcodex.app/pricing",
    });
  });

  it("外站地址一律退回默认落点", () => {
    expect(redirectTarget("https://evil.com/steal", "/account")).toEqual({
      kind: "internal",
      path: "/account",
    });
    expect(redirectTarget(null, "/account")).toEqual({ kind: "internal", path: "/account" });
  });

  it("在遗留门户主机上，站内 next 改走规范账号主机", () => {
    expect(
      redirectTarget("/account", "/account", { currentOrigin: "https://lumiogame.com" }),
    ).toEqual({
      kind: "external",
      url: "https://bestcodex.app/account",
    });
  });

  it("跨官方入口跳转时把令牌放进 hash", () => {
    const target = redirectTarget("https://bestcodex.app/codex", "/account", {
      currentOrigin: "https://lumiogame.com",
      tokens: { accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 },
    });
    expect(target.kind).toBe("external");
    if (target.kind !== "external") return;
    expect(target.url).toContain("https://bestcodex.app/codex");
    expect(target.url).toContain("lumio_at=at-1");
    expect(target.url).toContain("lumio_rt=rt-1");
  });

  it("外站 next 被丢弃后，令牌只跟去规范账号主机", () => {
    const target = redirectTarget("https://evil.com/steal", "/account", {
      currentOrigin: "https://lumiogame.com",
      tokens: { accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 },
    });
    expect(target.kind).toBe("external");
    if (target.kind !== "external") return;
    expect(target.url.startsWith("https://bestcodex.app/account")).toBe(true);
    expect(target.url).toContain("lumio_at=at-1");
    expect(target.url).not.toContain("evil.com");
  });
});

describe("isAllowedDesktopRedirect", () => {
  it("放行桌面端注册的三种回调形态", () => {
    expect(isAllowedDesktopRedirect("http://127.0.0.1:53682/callback")).toBe(true);
    expect(isAllowedDesktopRedirect("http://localhost:53682/callback")).toBe(true);
    expect(isAllowedDesktopRedirect("cchaven://auth/callback")).toBe(true);
  });

  it("授权码与 state 拼在查询串上不影响判定", () => {
    expect(isAllowedDesktopRedirect("http://127.0.0.1:53682/callback?code=abc&state=xyz")).toBe(
      true,
    );
  });

  it("拒绝一切不是本机回调的地址，防开放重定向", () => {
    for (const uri of [
      "https://evil.com/callback",
      "http://evil.com/callback",
      "http://127.0.0.1.evil.com/callback",
      "http://127.0.0.1:53682/steal",
      "https://127.0.0.1:53682/callback",
      "cchaven://evil/callback",
      "javascript:alert(1)",
      "//127.0.0.1:53682/callback",
      "",
      null,
    ]) {
      expect(isAllowedDesktopRedirect(uri), `${uri} 不应放行`).toBe(false);
    }
  });
});

describe("denyRedirectUrl", () => {
  it("按 OAuth 契约回跳 access_denied，并原样带回 state", () => {
    const url = new URL(denyRedirectUrl("http://127.0.0.1:53682/callback", "st4te") ?? "");

    expect(url.origin + url.pathname).toBe("http://127.0.0.1:53682/callback");
    expect(url.searchParams.get("error")).toBe("access_denied");
    expect(url.searchParams.get("error_description")).toBeTruthy();
    expect(url.searchParams.get("state")).toBe("st4te");
  });

  it("没有 state 时不产生空参数", () => {
    const url = new URL(denyRedirectUrl("cchaven://auth/callback", "") ?? "");

    expect(url.searchParams.has("state")).toBe(false);
  });

  it("回调地址不可信时不生成任何跳转", () => {
    expect(denyRedirectUrl("https://evil.com/callback", "st4te")).toBeNull();
  });
});
