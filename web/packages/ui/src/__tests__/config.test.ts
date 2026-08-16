import { afterEach, describe, expect, it, vi } from "vitest";

import {
  apiBaseUrl,
  ccControlBaseUrl,
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
  it("三站默认落在 bestcodex.app 及其子域", () => {
    expect(siteUrl("portal")).toBe("https://bestcodex.app");
    expect(siteUrl("cc")).toBe("https://cc.bestcodex.app");
    expect(siteUrl("codex")).toBe("https://codex.bestcodex.app");
  });

  it("环境变量可整体覆盖，便于本地起三个端口联调", () => {
    vi.stubEnv("VITE_PORTAL_URL", "http://localhost:5280/");
    expect(siteUrl("portal")).toBe("http://localhost:5280");
  });

  it("充值地址跟随 API base，默认是生产 Sub2API", () => {
    expect(apiBaseUrl()).toBe("https://api.lumio.games");
    expect(purchaseUrl()).toBe("https://api.lumio.games/purchase");
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
  it("接受站内相对路径与 bestcodex.app 子域", () => {
    expect(isAllowedNext("/account")).toBe(true);
    expect(isAllowedNext("https://cc.bestcodex.app/pricing")).toBe(true);
    expect(isAllowedNext("https://bestcodex.app/")).toBe(true);
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
