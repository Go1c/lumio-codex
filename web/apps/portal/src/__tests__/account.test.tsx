import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { clearSession, writeSession } from "@lumio/auth";

import { accountTabFromHash } from "@/lib/accountTabs";
import { PROFILE, ccEnvelope, envelope, renderApp, stubFetch } from "@/test/utils";

beforeEach(() => {
  clearSession();
});

afterEach(() => {
  clearSession();
  vi.unstubAllGlobals();
});

describe("账户中心页签 hash", () => {
  it("把 #orders 和旧锚点映射到开通记录，空 hash 落账户", () => {
    expect(accountTabFromHash("")).toBe("profile");
    expect(accountTabFromHash("#orders")).toBe("orders");
    expect(accountTabFromHash("#claude-orders")).toBe("orders");
    expect(accountTabFromHash("#balance")).toBe("balance");
    expect(accountTabFromHash("#affiliate")).toBe("affiliate");
  });
});

describe("账户中心", () => {
  it("未登录时引导去登录，不请求账户接口", async () => {
    const fetchMock = stubFetch({});

    renderApp("/account");

    expect(await screen.findByRole("link", { name: "去登录" })).toHaveAttribute("href", "/login");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("已登录时用页签分栏，默认账户页签展示邮箱与状态", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    stubFetch({ "/auth/me": () => envelope(PROFILE) });

    renderApp("/account");

    expect(await screen.findByText("user@example.com")).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "账户" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tab", { name: "余额" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "开通记录" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "邀请返利" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /充值/ })).not.toBeInTheDocument();
    expect(
      screen.getAllByRole("link", { name: "BestCodex" }).some(
        (link) => link.getAttribute("href") === "https://bestcodex.app/codex",
      ),
    ).toBe(true);
    expect(screen.getAllByText(/一个启动器/).length).toBeGreaterThan(0);
    expect(screen.queryByText("Lumio Codex")).not.toBeInTheDocument();
    expect(screen.queryByText("CC避风港")).not.toBeInTheDocument();
  });

  it("余额页签展示余额，充值跳 Sub2API 收银台", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    stubFetch({
      "/auth/me": () => envelope(PROFILE),
      "/user/balance/transactions": () => envelope({ items: [] }),
    });

    renderApp("/account");
    await userEvent.click(await screen.findByRole("tab", { name: "余额" }));

    expect(screen.getByText("¥12.50")).toBeInTheDocument();
    const topup = screen.getByRole("link", { name: /充值/ });
    expect(topup.getAttribute("href")).toMatch(/^https:\/\/api\.lumio\.games\/auth\/bridge#/);
    const hash = new URL(topup.getAttribute("href") ?? "").hash.slice(1);
    const params = new URLSearchParams(hash);
    expect(params.get("t")).toBe("at-1");
    expect(params.get("r")).toBe("/purchase");
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

const ACTIVE_ENTITLEMENT = {
  status: "active",
  kind: "paid",
  expires_at: "2026-09-18T00:00:00Z",
  days_left: 30,
  bonus_days_total: 0,
  expiring_soon: false,
};

const NONE_ENTITLEMENT = {
  status: "none",
  days_left: 0,
  bonus_days_total: 0,
  expiring_soon: false,
};

const EXPIRED_ENTITLEMENT = {
  status: "expired",
  kind: "paid",
  expires_at: "2026-07-01T00:00:00Z",
  days_left: 0,
  bonus_days_total: 0,
  expiring_soon: false,
};

const PENDING_ORDER = {
  order_no: "CC20260819-000001",
  amount_cents: 1990,
  currency: "CNY",
  channel: "balance",
  status: "pending",
  created_at: "2026-08-19T00:00:00Z",
};

const PAID_ORDER = {
  order_no: "CC20260819-000002",
  amount_cents: 1990,
  currency: "CNY",
  channel: "balance",
  status: "paid",
  paid_at: "2026-08-19T00:01:00Z",
  created_at: "2026-08-19T00:00:00Z",
};

function accountHandlers(extra: Record<string, (init?: RequestInit) => Response> = {}) {
  return {
    "/auth/me": () => envelope(PROFILE),
    "/me/entitlement": () => ccEnvelope(NONE_ENTITLEMENT),
    "/billing/orders": () => ccEnvelope({ items: [] }),
    "/user/balance/transactions": () => envelope({ items: [] }),
    "/user/aff": () => envelope({ ...AFF_DETAIL, rules: undefined }),
    ...extra,
  };
}

describe("账户中心 · Claude 订阅与账单", () => {
  it("默认账户页签不展示开通记录，#orders 直接打开开通记录页签", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    stubFetch(
      accountHandlers({
        "/billing/orders": () => ccEnvelope({ items: [PENDING_ORDER] }),
      }),
    );

    renderApp("/account");

    expect(await screen.findByText("user@example.com")).toBeInTheDocument();
    expect(screen.queryByText("CC20260819-000001")).not.toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "开通记录" })).toHaveAttribute("aria-selected", "false");

    await userEvent.click(screen.getByRole("tab", { name: "开通记录" }));

    expect(screen.getByRole("tab", { name: "开通记录" })).toHaveAttribute("aria-selected", "true");
    expect(await screen.findByText("CC20260819-000001")).toBeInTheDocument();
    expect(screen.queryByText("user@example.com")).not.toBeInTheDocument();
  });

  it("hash #orders 落地开通记录页签", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    stubFetch(
      accountHandlers({
        "/billing/orders": () => ccEnvelope({ items: [PENDING_ORDER, PAID_ORDER] }),
      }),
    );

    renderApp("/account#orders");

    expect(await screen.findByRole("tab", { name: "开通记录" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(await screen.findByText("CC20260819-000001")).toBeInTheDocument();
  });

  it("active 显示有效期至本地日期和剩余天数", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    stubFetch(accountHandlers({ "/me/entitlement": () => ccEnvelope(ACTIVE_ENTITLEMENT) }));

    renderApp("/account#orders");

    expect(await screen.findByText(/已订阅 · 有效期至/)).toBeInTheDocument();
    expect(screen.getByText(/剩余 30 天/)).toBeInTheDocument();
    expect(screen.queryByText("2026-09-18T00:00:00Z")).not.toBeInTheDocument();
  });

  it("none 显示未订阅，并说明到桌面开通而不是在门户扣款", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    stubFetch(accountHandlers());

    renderApp("/account#orders");

    expect(await screen.findByText("未订阅")).toBeInTheDocument();
    expect(screen.getByText(/BestCodex 桌面 Claude Tab 用余额开通/)).toBeInTheDocument();
    expect(screen.queryByText(/用余额支付/)).not.toBeInTheDocument();
  });

  it("过期显示已过期", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    stubFetch(accountHandlers({ "/me/entitlement": () => ccEnvelope(EXPIRED_ENTITLEMENT) }));

    renderApp("/account#orders");

    expect(await screen.findByText("订阅已过期")).toBeInTheDocument();
  });

  it("有 pending 与 paid 单时能看到开通记录", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    stubFetch(
      accountHandlers({
        "/billing/orders": () => ccEnvelope({ items: [PENDING_ORDER, PAID_ORDER] }),
      }),
    );

    renderApp("/account#orders");

    expect(await screen.findByText("CC20260819-000001")).toBeInTheDocument();
    expect(screen.getByText("CC20260819-000002")).toBeInTheDocument();
    expect(screen.getAllByText(/¥19.90/).length).toBeGreaterThan(0);
    expect(screen.getByText(/处理中，请勿重复支付/)).toBeInTheDocument();
    expect(screen.getByText(/已支付/)).toBeInTheDocument();
  });

  it("pending 订单与未开通同时存在时写明不要重复支付", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    stubFetch(
      accountHandlers({
        "/billing/orders": () => ccEnvelope({ items: [PENDING_ORDER] }),
      }),
    );

    renderApp("/account#orders");

    expect(await screen.findByText("未订阅")).toBeInTheDocument();
    expect(screen.getByText(/钱可能已扣、权益尚未到账/)).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "刷新开通状态" }).length).toBeGreaterThan(0);
  });

  it("处理中的余额单可以刷新开通状态，走原单 resume 而不是新支付", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    let pending = true;
    const fetchMock = stubFetch(
      accountHandlers({
        "/me/entitlement": () => ccEnvelope(pending ? NONE_ENTITLEMENT : ACTIVE_ENTITLEMENT),
        "/billing/orders": () =>
          ccEnvelope({
            items: [pending ? PENDING_ORDER : { ...PENDING_ORDER, status: "paid", paid_at: PAID_ORDER.paid_at }],
          }),
        "/billing/orders/CC20260819-000001/resume": (init?: RequestInit) => {
          expect(init?.method).toBe("POST");
          expect(JSON.stringify(init?.headers ?? {})).not.toMatch(/Idempotency-Key/i);
          pending = false;
          return ccEnvelope({
            order: { ...PENDING_ORDER, status: "paid", paid_at: PAID_ORDER.paid_at },
            entitlement: ACTIVE_ENTITLEMENT,
          });
        },
      }),
    );

    renderApp("/account#orders");

    const refresh = await screen.findAllByRole("button", { name: "刷新开通状态" });
    expect(refresh.length).toBeGreaterThan(0);
    await userEvent.click(refresh[0]);

    expect(await screen.findByText(/已订阅 · 有效期至/)).toBeInTheDocument();
    expect(screen.getByText(/已支付/)).toBeInTheDocument();
    expect(screen.queryByText(/处理中，请勿重复支付/)).not.toBeInTheDocument();
    const resumeCalls = fetchMock.mock.calls.filter(([url]) =>
      String(url).includes("/billing/orders/CC20260819-000001/resume"),
    );
    expect(resumeCalls).toHaveLength(1);
    expect(String(resumeCalls[0]?.[0])).toContain("/billing/orders/CC20260819-000001/resume");
    expect(fetchMock.mock.calls.some(([url]) => String(url).includes("pay-with-balance"))).toBe(false);
  });
});

describe("账户中心 · 邀请返利", () => {
  it("展示邀请码、额度与 rules 驱动的规则文案，被邀邮箱保持服务端脱敏形态", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    stubFetch({
      "/auth/me": () => envelope(PROFILE),
      "/user/aff": () => envelope(AFF_DETAIL),
    });

    renderApp("/account#affiliate");

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

    renderApp("/account#affiliate");

    expect(await screen.findByText("39XZR7KLHECZ")).toBeInTheDocument();
    expect(screen.queryByText(/小时后可划转/)).not.toBeInTheDocument();
    expect(screen.queryByText(/天内的充值计入返利/)).not.toBeInTheDocument();
  });

  it("当前 1% 且未满足 L2 充值门槛时，不谎称已达到 3%", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    stubFetch({
      "/auth/me": () => envelope(PROFILE),
      "/user/aff": () =>
        envelope({
          ...AFF_DETAIL,
          aff_count: 2,
          invitee_recharge_total: 0,
          effective_rebate_rate_percent: 1,
          current_affiliate_tier: {
            level: "L1",
            min_invitees: 0,
            min_recharge: 0,
            rebate_rate_percent: 1,
          },
          next_affiliate_tier: {
            level: "L2",
            min_invitees: 2,
            min_recharge: 100,
            rebate_rate_percent: 3,
          },
        }),
    });

    renderApp("/account#affiliate");

    expect(await screen.findByText(/当前比例 1%/)).toBeInTheDocument();
    expect(screen.getByText(/好友再累计充值/)).toBeInTheDocument();
    expect(screen.getByText(/升至 L2（比例升至 3%）/)).toBeInTheDocument();
    expect(screen.queryByText(/已达到 L2/)).not.toBeInTheDocument();
  });

  it("划转到余额成功后给出提示并刷新余额", async () => {
    const fetchMock = stubFetch({
      "/aff/transfer": () => envelope({ transferred_quota: 5.5, balance: 18 }),
      "/auth/me": () => envelope(PROFILE),
      "/user/aff": () => envelope(AFF_DETAIL),
    });

    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    renderApp("/account#affiliate");

    await userEvent.click(await screen.findByRole("button", { name: "划转到余额" }));

    expect(await screen.findByText("已划转 ¥5.50 到账户余额。")).toBeInTheDocument();
    const meCalls = fetchMock.mock.calls.filter(([url]) => String(url).includes("/auth/me"));
    expect(meCalls.length).toBeGreaterThanOrEqual(2);
  });
});
