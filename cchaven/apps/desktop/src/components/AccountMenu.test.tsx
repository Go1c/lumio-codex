import { screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AccountMenu, entitlementChipLine, entitlementLine, isExpiringSoon } from "./AccountMenu";
import { renderWithProviders } from "../test/render";
import type { Entitlement, ExternalLinks } from "../lib/types";

const LINKS: ExternalLinks = {
  account: "https://cchaven.cn/account",
  invite: "https://cchaven.cn/account#invite",
  docs: "https://cchaven.cn/docs",
  support: "https://cchaven.cn/support",
  serverGuide: "https://cchaven.cn/docs/buy-a-server",
  troubleshooting: "https://cchaven.cn/docs/connection-troubleshooting",
};

function entitlement(overrides: Partial<Entitlement> = {}): Entitlement {
  return {
    status: "active",
    kind: "monthly",
    expiresAt: "2026-09-08T00:00:00Z",
    daysLeft: 27,
    expiringSoon: false,
    ...overrides,
  };
}

function setup(overrides: Partial<Entitlement> = {}) {
  const onOpenExternal = vi.fn();
  const onLogout = vi.fn();
  const harness = renderWithProviders(
    <AccountMenu
      session={{ email: "mary@example.com", entitlement: entitlement(overrides) }}
      links={LINKS}
      onOpenExternal={onOpenExternal}
      onLogout={onLogout}
      onClose={vi.fn()}
    />,
  );
  return { ...harness, onOpenExternal, onLogout };
}

describe("账户菜单（5.6）", () => {
  it("订阅状态行使用 YYYY年M月D日 日期格式", () => {
    expect(entitlementLine(entitlement())).toBe("已订阅 · 有效期至 2026年9月8日（剩余 27 天）");
    expect(entitlementLine(entitlement({ status: "trialing", daysLeft: 23 }))).toBe(
      "免费试用中 · 剩余 23 天",
    );
    expect(entitlementLine({ status: "none", daysLeft: 0 })).toBe("未订阅");
    expect(entitlementChipLine(entitlement())).toBe("已订阅 · 剩 27 天");
  });

  it("剩余 ≤3 天判定为即将到期", () => {
    expect(isExpiringSoon(entitlement({ daysLeft: 4 }))).toBe(false);
    expect(isExpiringSoon(entitlement({ daysLeft: 3 }))).toBe(true);
    expect(isExpiringSoon(entitlement({ daysLeft: 10, expiringSoon: true }))).toBe(true);
    expect(isExpiringSoon(null)).toBe(false);
  });

  it("前四项跳官网，菜单里没有任何付款入口", async () => {
    const harness = setup();

    const labels = screen.getAllByRole("menuitem").map((item) => item.textContent ?? "");
    expect(labels).toEqual([
      "🌐管理订阅与账号 ↗",
      "🎁邀请好友 ↗",
      "📖使用文档 ↗",
      "💬联系我们 ↗",
      "↩退出登录",
    ]);
    expect(screen.queryByText(/支付|付款|信用卡/)).not.toBeInTheDocument();

    await harness.user.click(screen.getByRole("menuitem", { name: /管理订阅与账号/ }));
    expect(harness.onOpenExternal).toHaveBeenCalledWith("https://cchaven.cn/account");
  });

  it("退出登录交给上层清钥匙串并撤销会话", async () => {
    const harness = setup();
    await harness.user.click(screen.getByRole("menuitem", { name: /退出登录/ }));
    expect(harness.onLogout).toHaveBeenCalled();
  });

  it("即将到期时订阅状态行转为橙色样式", () => {
    setup({ daysLeft: 2 });
    expect(screen.getByText("已订阅 · 有效期至 2026年9月8日（剩余 2 天）")).toHaveClass(
      "plan-line",
      "warn",
    );
  });
});
