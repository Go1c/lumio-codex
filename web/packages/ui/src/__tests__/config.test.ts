import { afterEach, describe, expect, it, vi } from "vitest";

import {
  apiBaseUrl,
  bounceToCanonicalUrl,
  ccControlBaseUrl,
  cookieDomainFor,
  helpProductUrl,
  isAllowedNext,
  isOfficialAccountHost,
  portalAccountLinks,
  portalUrl,
  purchaseUrl,
  resolveNext,
  shouldBounceToCanonical,
  siteUrl,
} from "../config";

afterEach(() => {
  vi.unstubAllEnvs();
});

describe("站点地址", () => {
  it("产品站默认是单站 + 路由，不是子域", () => {
    expect(siteUrl("portal")).toBe("https://bestcodex.app");
    expect(siteUrl("codex")).toBe("https://bestcodex.app/codex");
    expect(siteUrl("cc")).toBe("https://bestcodex.app/claude");
  });

  it("环境变量可整体覆盖，便于本地起两个端口联调", () => {
    vi.stubEnv("VITE_PORTAL_URL", "http://localhost:5280/");
    vi.stubEnv("VITE_CODEX_URL", "http://localhost:5282/codex");
    vi.stubEnv("VITE_CC_URL", "http://localhost:5282/claude");
    expect(siteUrl("portal")).toBe("http://localhost:5280");
    expect(siteUrl("codex")).toBe("http://localhost:5282/codex");
    expect(siteUrl("cc")).toBe("http://localhost:5282/claude");
  });

  it("根域覆盖会改默认产品站路由，不改 Sub2API", () => {
    vi.stubEnv("VITE_ROOT_DOMAIN", "example.test");
    expect(siteUrl("codex")).toBe("https://example.test/codex");
    expect(siteUrl("cc")).toBe("https://example.test/claude");
    expect(apiBaseUrl()).toBe("https://api.lumio.games");
    expect(purchaseUrl()).toBe("https://api.lumio.games/purchase");
  });

  it("产品站帮助落在 apex /help，不拼到 /codex/help", () => {
    expect(helpProductUrl()).toBe("https://bestcodex.app/help");
  });

  it("充值地址跟随 API base，默认是生产 Sub2API", () => {
    expect(apiBaseUrl()).toBe("https://api.lumio.games");
    expect(purchaseUrl()).toBe("https://api.lumio.games/purchase");
  });

  it("有 access token 时走 /auth/bridge，令牌只放 hash，不带 refresh", () => {
    const url = purchaseUrl("at-1");
    expect(url.startsWith("https://api.lumio.games/auth/bridge#")).toBe(true);
    const hash = new URL(url).hash.slice(1);
    const params = new URLSearchParams(hash);
    expect(params.get("t")).toBe("at-1");
    expect(params.get("r")).toBe("/purchase");
    expect(params.has("rt")).toBe(false);
    expect(url).not.toContain("refresh");
  });

  it("空 token 仍直开收银台，避免交出半截交接地址", () => {
    expect(purchaseUrl(null)).toBe("https://api.lumio.games/purchase");
    expect(purchaseUrl("")).toBe("https://api.lumio.games/purchase");
    expect(purchaseUrl("   ")).toBe("https://api.lumio.games/purchase");
  });

  it("CC 控制面地址独立于 Sub2API，可用环境变量覆盖", () => {
    expect(ccControlBaseUrl()).toBe("https://api.cc.bestcodex.app");

    vi.stubEnv("VITE_CC_CONTROL_URL", "http://localhost:8080/");
    expect(ccControlBaseUrl()).toBe("http://localhost:8080");
  });
});

describe("门户回跳链接", () => {
  it("账号入口一律指向门户，并带上 next 回跳参数", () => {
    const links = portalAccountLinks("https://cc.bestcodex.app/pricing");

    expect(links.login).toBe(
      "https://bestcodex.app/login?next=https%3A%2F%2Fcc.bestcodex.app%2Fpricing",
    );
    expect(links.signup).toBe(
      "https://bestcodex.app/signup?next=https%3A%2F%2Fcc.bestcodex.app%2Fpricing",
    );
    expect(links.account).toBe(
      "https://bestcodex.app/account?next=https%3A%2F%2Fcc.bestcodex.app%2Fpricing",
    );
  });

  it("没有回跳目标时不产生空的 next 参数", () => {
    expect(portalUrl("/login")).toBe("https://bestcodex.app/login");
    expect(portalUrl("/login", "")).toBe("https://bestcodex.app/login");
  });
});

describe("回跳目标校验", () => {
  it("接受站内相对路径、bestcodex.app 子域与遗留门户 lumiogame.com", () => {
    expect(isAllowedNext("/account")).toBe(true);
    expect(isAllowedNext("https://cc.bestcodex.app/pricing")).toBe(true);
    expect(isAllowedNext("https://bestcodex.app/")).toBe(true);
    expect(isAllowedNext("https://lumiogame.com/account")).toBe(true);
  });

  it("拒绝外站、协议相对地址与伪协议，避免开放重定向", () => {
    expect(isAllowedNext("https://evil.com/steal")).toBe(false);
    expect(isAllowedNext("//evil.com")).toBe(false);
    expect(isAllowedNext("javascript:alert(1)")).toBe(false);
    expect(isAllowedNext("https://bestcodex.app.evil.com/")).toBe(false);
    expect(isAllowedNext("")).toBe(false);
    expect(isAllowedNext(null)).toBe(false);
  });

  it("非法目标退回默认落点", () => {
    expect(resolveNext("https://evil.com", "/account")).toBe("/account");
    expect(resolveNext("/account", "/")).toBe("/account");
  });
});

describe("遗留账号入口回跳", () => {
  it("lumiogame.com 是官方入口，但不是规范账号主机", () => {
    expect(isOfficialAccountHost("lumiogame.com")).toBe(true);
    expect(isOfficialAccountHost("bestcodex.app")).toBe(true);
    expect(isOfficialAccountHost("evil.com")).toBe(false);
    expect(shouldBounceToCanonical("lumiogame.com")).toBe(true);
    expect(shouldBounceToCanonical("bestcodex.app")).toBe(false);
    expect(shouldBounceToCanonical("localhost")).toBe(false);
  });

  it("把遗留主机的路径搬到规范账号 origin，保留查询串", () => {
    expect(bounceToCanonicalUrl("https://lumiogame.com/account")).toBe(
      "https://bestcodex.app/account",
    );
    expect(bounceToCanonicalUrl("https://lumiogame.com/login?next=%2Faccount")).toBe(
      "https://bestcodex.app/login?next=%2Faccount",
    );
    expect(bounceToCanonicalUrl("https://bestcodex.app/account")).toBeNull();
    expect(bounceToCanonicalUrl("http://localhost:5280/login")).toBeNull();
  });
});

describe("会话 Cookie 作用域", () => {
  it("生产域下写父域 Cookie，三站共享同一会话", () => {
    expect(cookieDomainFor("bestcodex.app")).toBe(".bestcodex.app");
    expect(cookieDomainFor("cc.bestcodex.app")).toBe(".bestcodex.app");
  });

  it("开发环境退化为当前 host，不写 Domain", () => {
    expect(cookieDomainFor("localhost")).toBeUndefined();
    expect(cookieDomainFor("127.0.0.1")).toBeUndefined();
  });
});
