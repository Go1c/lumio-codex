import { screen, within } from "@testing-library/react";
import { http } from "msw";
import { describe, expect, it } from "vitest";

import { db } from "@/mocks/db";
import { fail } from "@/mocks/handlers";
import { server } from "@/mocks/server";
import { renderApp } from "@/test/utils";

/** 4.1 / 4.2 / 4.3：营销页的数值全部来自 `GET /config/public`，页面不写死。 */

const API = "*/api/v1";

describe("首页 /", () => {
  it("讲清两件事并给出下载与定价 CTA", async () => {
    renderApp("/");

    expect(await screen.findByRole("heading", { name: /安心使用 Claude Code/ })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "防封方案" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "双向安全同步" })).toBeInTheDocument();
    expect(screen.getAllByRole("link", { name: "下载 macOS 版" }).length).toBeGreaterThan(0);
    expect(screen.getByRole("link", { name: "查看定价" })).toHaveAttribute("href", "/pricing");
  });

  it("定价摘要展示后台下发的价格", async () => {
    db.config = { ...db.config, pricing: { amount_cents: 9900, currency: "CNY", period_unit: "month" } };
    renderApp("/");

    expect(await screen.findByText("¥99")).toBeInTheDocument();
  });
});

describe("定价页 /pricing", () => {
  it("加载中显示骨架卡（loading 态）", async () => {
    server.use(http.get(`${API}/config/public`, () => new Promise<never>(() => {})));
    renderApp("/pricing");

    expect(await screen.findByText("加载中…")).toBeInTheDocument();
  });

  it("价格加载失败显示重试块（error 态）", async () => {
    server.use(
      http.get(`${API}/config/public`, () => fail(500, "internal_error", "服务暂时不可用，请稍后重试。")),
    );
    renderApp("/pricing");

    expect(await screen.findByText("服务暂时不可用，请稍后重试。")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "重试" })).toBeInTheDocument();
  });

  it("未登录时 CTA 指向注册页", async () => {
    renderApp("/pricing");

    expect(await screen.findByRole("link", { name: "立即订阅" })).toHaveAttribute("href", "/signup");
    expect(screen.getByText("🎁 经朋友邀请注册并登录 APP，首月免费（每个账号限一次）。")).toBeInTheDocument();
  });

  it("已订阅时展示「已订阅」徽标", async () => {
    db.loggedIn = true;
    renderApp("/pricing");

    expect(await screen.findByText("已订阅")).toBeInTheDocument();
  });

  it("试用中展示剩余天数", async () => {
    db.loggedIn = true;
    db.entitlement = {
      status: "trialing",
      kind: "trial",
      days_left: 23,
      bonus_days_total: 0,
      expiring_soon: false,
    };
    renderApp("/pricing");

    expect(await screen.findByText("试用中 · 剩余 23 天")).toBeInTheDocument();
  });

  it("FAQ 手风琴同时只展开一项", async () => {
    const { user } = renderApp("/pricing");

    const first = await screen.findByRole("button", { name: /有没有免费版？/ });
    const second = screen.getByRole("button", { name: /订阅有什么限制？/ });

    await user.click(first);
    expect(first).toHaveAttribute("aria-expanded", "true");

    await user.click(second);
    expect(first).toHaveAttribute("aria-expanded", "false");
    expect(second).toHaveAttribute("aria-expanded", "true");
  });
});

describe("下载页 /download", () => {
  it("展示后台下发的版本号、更新日期与系统要求", async () => {
    renderApp("/download");

    expect(await screen.findByText("版本 1.4.2 · 2026年8月8日更新 · 需要 macOS 13 及以上")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "下载 macOS 版（Apple Silicon）" })).toHaveAttribute(
      "href",
      "https://dl.cchaven.cn/CCHaven-1.4.2-arm64.dmg",
    );
    expect(screen.getByRole("link", { name: "下载 Intel 版" })).toHaveAttribute(
      "href",
      "https://dl.cchaven.cn/CCHaven-1.4.2-x64.dmg",
    );
  });

  it("没有可下载版本时显示空状态（empty 态）", async () => {
    db.config = { ...db.config, releases: [] };
    renderApp("/download");

    expect(await screen.findByText("暂无可下载的版本，请稍后再来。")).toBeInTheDocument();
  });

  it("配置加载失败时显示错误条与重试（error 态）", async () => {
    server.use(
      http.get(`${API}/config/public`, () => fail(503, "internal_error", "服务暂时不可用，请稍后重试。")),
    );
    renderApp("/download");

    const alert = await screen.findByRole("alert");
    expect(within(alert).getByRole("button", { name: "重试" })).toBeInTheDocument();
  });
});
