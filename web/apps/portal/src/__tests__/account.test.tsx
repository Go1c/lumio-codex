import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { clearSession, writeSession } from "@lumio/auth";

import { PROFILE, envelope, renderApp, stubFetch } from "@/test/utils";

beforeEach(() => {
  clearSession();
});

afterEach(() => {
  clearSession();
  vi.unstubAllGlobals();
});

describe("账户中心", () => {
  it("未登录时引导去登录，不请求账户接口", async () => {
    const fetchMock = stubFetch({});

    renderApp("/account");

    expect(await screen.findByRole("link", { name: "去登录" })).toHaveAttribute("href", "/login");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("已登录时展示邮箱、余额与状态，充值跳 Sub2API 收银台", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    stubFetch({ "/auth/me": () => envelope(PROFILE) });

    renderApp("/account");

    expect(await screen.findByText("user@example.com")).toBeInTheDocument();
    expect(screen.getByText("¥12.50")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /充值/ })).toHaveAttribute(
      "href",
      "https://api.lumio.games/purchase",
    );
  });

  it("登出后清掉共享会话 Cookie", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    stubFetch({
      "/auth/me": () => envelope(PROFILE),
      "/auth/logout": () => envelope({}),
    });

    renderApp("/account");
    await userEvent.click(await screen.findByRole("button", { name: "退出登录" }));

    await waitFor(() => expect(document.cookie).not.toContain("lumio_at"));
    expect(await screen.findByRole("link", { name: "去登录" })).toBeInTheDocument();
  });
});

const AFF_DETAIL = {
  user_id: 7,
  aff_code: "39XZR7KLHECZ",
  aff_count: 2,
  aff_quota: 5.5,
  aff_frozen_quota: 1.25,
  aff_history_quota: 8.75,
  invitee_recharge_total: 300,
  effective_rebate_rate_percent: 3,
  affiliate_tiers: [
    { level: "L1", min_invitees: 0, min_recharge: 0, rebate_rate_percent: 1 },
    { level: "L2", min_invitees: 2, min_recharge: 100, rebate_rate_percent: 3 },
  ],
  invitees: [
    {
      user_id: 170,
      email: "c***@i***.com",
      username: "",
      created_at: "2026-04-26T23:19:25Z",
      total_rebate: 0,
    },
  ],
  rules: {
    rebate_freeze_hours: 24,
    rebate_duration_days: 60,
    rebate_per_invitee_cap: 100,
    signup_bonus_enabled: true,
    signup_bonus_amount: 0.99,
  },
};

describe("账户中心 · 邀请返利", () => {
  it("展示邀请码、额度与 rules 驱动的规则文案，被邀邮箱保持服务端脱敏形态", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    stubFetch({
      "/auth/me": () => envelope(PROFILE),
      "/user/aff": () => envelope(AFF_DETAIL),
    });

    renderApp("/account");

    expect(await screen.findByText("39XZR7KLHECZ")).toBeInTheDocument();
    expect(screen.getByText("¥5.50")).toBeInTheDocument();
    expect(screen.getByText(/冻结 24 小时后可划转/)).toBeInTheDocument();
    expect(screen.getByText(/60 天内的充值计入返利/)).toBeInTheDocument();
    expect(screen.getByText("c***@i***.com")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "复制邀请链接" })).toBeInTheDocument();
  });

  it("rules 缺失（后端未部署）时隐藏规则文案，其余照常展示", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    // rules: undefined 在 JSON 序列化时被丢弃，模拟旧后端没有该键。
    const withoutRules = { ...AFF_DETAIL, rules: undefined };
    stubFetch({
      "/auth/me": () => envelope(PROFILE),
      "/user/aff": () => envelope(withoutRules),
    });

    renderApp("/account");

    expect(await screen.findByText("39XZR7KLHECZ")).toBeInTheDocument();
    expect(screen.queryByText(/小时后可划转/)).not.toBeInTheDocument();
    expect(screen.queryByText(/天内的充值计入返利/)).not.toBeInTheDocument();
  });

  it("划转到余额成功后给出提示并刷新余额", async () => {
    const fetchMock = stubFetch({
      "/aff/transfer": () => envelope({ transferred_quota: 5.5, balance: 18 }),
      "/auth/me": () => envelope(PROFILE),
      "/user/aff": () => envelope(AFF_DETAIL),
    });

    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    renderApp("/account");

    await userEvent.click(await screen.findByRole("button", { name: "划转到余额" }));

    expect(await screen.findByText("已划转 ¥5.50 到账户余额。")).toBeInTheDocument();
    const meCalls = fetchMock.mock.calls.filter(([url]) => String(url).includes("/auth/me"));
    expect(meCalls.length).toBeGreaterThanOrEqual(2);
  });
});
