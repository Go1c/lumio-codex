import { afterEach, describe, expect, it, vi } from "vitest";

import {
  apiBaseUrl,
  cookieDomainFor,
  isAllowedNext,
  portalAccountLinks,
  portalUrl,
  purchaseUrl,
  resolveNext,
  siteUrl,
} from "../config";

afterEach(() => {
  vi.unstubAllEnvs();
});

describe("站点地址", () => {
  it("三站默认落在 lumiogame.com 及其子域", () => {
    expect(siteUrl("portal")).toBe("https://lumiogame.com");
    expect(siteUrl("cc")).toBe("https://cc.lumiogame.com");
    expect(siteUrl("codex")).toBe("https://codex.lumiogame.com");
  });

  it("环境变量可整体覆盖，便于本地起三个端口联调", () => {
    vi.stubEnv("VITE_PORTAL_URL", "http://localhost:5280/");
    expect(siteUrl("portal")).toBe("http://localhost:5280");
  });

  it("充值地址跟随 API base，默认是生产 Sub2API", () => {
    expect(apiBaseUrl()).toBe("https://api.lumio.games");
    expect(purchaseUrl()).toBe("https://api.lumio.games/purchase");
  });
});

describe("门户回跳链接", () => {
  it("账号入口一律指向门户，并带上 next 回跳参数", () => {
    const links = portalAccountLinks("https://cc.lumiogame.com/pricing");

    expect(links.login).toBe(
      "https://lumiogame.com/login?next=https%3A%2F%2Fcc.lumiogame.com%2Fpricing",
    );
    expect(links.signup).toBe(
      "https://lumiogame.com/signup?next=https%3A%2F%2Fcc.lumiogame.com%2Fpricing",
    );
    expect(links.account).toBe(
      "https://lumiogame.com/account?next=https%3A%2F%2Fcc.lumiogame.com%2Fpricing",
    );
  });

  it("没有回跳目标时不产生空的 next 参数", () => {
    expect(portalUrl("/login")).toBe("https://lumiogame.com/login");
    expect(portalUrl("/login", "")).toBe("https://lumiogame.com/login");
  });
});

describe("回跳目标校验", () => {
  it("接受站内相对路径与 lumiogame.com 子域", () => {
    expect(isAllowedNext("/account")).toBe(true);
    expect(isAllowedNext("https://cc.lumiogame.com/pricing")).toBe(true);
    expect(isAllowedNext("https://lumiogame.com/")).toBe(true);
  });

  it("拒绝外站、协议相对地址与伪协议，避免开放重定向", () => {
    expect(isAllowedNext("https://evil.com/steal")).toBe(false);
    expect(isAllowedNext("//evil.com")).toBe(false);
    expect(isAllowedNext("javascript:alert(1)")).toBe(false);
    expect(isAllowedNext("https://lumiogame.com.evil.com/")).toBe(false);
    expect(isAllowedNext("")).toBe(false);
    expect(isAllowedNext(null)).toBe(false);
  });

  it("非法目标退回默认落点", () => {
    expect(resolveNext("https://evil.com", "/account")).toBe("/account");
    expect(resolveNext("/account", "/")).toBe("/account");
  });
});

describe("会话 Cookie 作用域", () => {
  it("生产域下写父域 Cookie，三站共享同一会话", () => {
    expect(cookieDomainFor("lumiogame.com")).toBe(".lumiogame.com");
    expect(cookieDomainFor("cc.lumiogame.com")).toBe(".lumiogame.com");
  });

  it("开发环境退化为当前 host，不写 Domain", () => {
    expect(cookieDomainFor("localhost")).toBeUndefined();
    expect(cookieDomainFor("127.0.0.1")).toBeUndefined();
  });
});
