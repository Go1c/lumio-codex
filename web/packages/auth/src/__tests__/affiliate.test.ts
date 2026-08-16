import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  LumioApiError,
  fetchAffiliate,
  fetchAffiliateLogs,
  register,
  transferAffiliateQuota,
} from "../client";

/** 契约口径见仓库 docs/ops/05-sub2api-affiliate-contract.md（§3 端点表与 §3.1 rules）。 */

const fetchMock = vi.fn();

function envelope(data: unknown, status = 200) {
  return new Response(JSON.stringify({ code: 0, message: "success", data }), { status });
}

function failure(status: number, reason: string) {
  return new Response(JSON.stringify({ code: status, message: "服务端原文", reason }), { status });
}

const DETAIL = {
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
    { level: "L2", min_invitees: 2, min_recharge: 100, rebate_rate_percent: null },
  ],
  current_affiliate_tier: { level: "L1", min_invitees: 0, min_recharge: 0, rebate_rate_percent: 1 },
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

beforeEach(() => {
  fetchMock.mockReset();
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("GET /user/aff", () => {
  it("映射详情字段、阶梯与运行规则，带 Bearer 令牌", async () => {
    fetchMock.mockResolvedValue(envelope(DETAIL));

    const detail = await fetchAffiliate("at-1");

    expect(fetchMock.mock.calls[0][0]).toBe("https://api.lumio.games/api/v1/user/aff");
    const headers = fetchMock.mock.calls[0][1]?.headers as Record<string, string>;
    expect(headers.Authorization).toBe("Bearer at-1");

    expect(detail.affCode).toBe("39XZR7KLHECZ");
    expect(detail.affQuota).toBe(5.5);
    expect(detail.effectiveRebateRatePercent).toBe(3);
    expect(detail.tiers[1].rebateRatePercent).toBeNull();
    expect(detail.invitees[0].email).toBe("c***@i***.com");
    expect(detail.rules).toEqual({
      rebateFreezeHours: 24,
      rebateDurationDays: 60,
      rebatePerInviteeCap: 100,
      signupBonusEnabled: true,
      signupBonusAmount: 0.99,
    });
  });

  it("rules 缺失（后端未部署 PR #307）→ null，不冒充全 0", async () => {
    fetchMock.mockResolvedValue(envelope({ aff_code: "ABCDEF123456" }));

    const detail = await fetchAffiliate("at-1");

    expect(detail.rules).toBeNull();
    expect(detail.affQuota).toBe(0);
    expect(detail.currentTier).toBeNull();
    expect(detail.invitees).toEqual([]);
  });
});

describe("GET /user/aff/logs", () => {
  it("映射分页信封", async () => {
    fetchMock.mockResolvedValue(
      envelope({
        items: [
          {
            id: 1,
            invitee_email: "m***@g***.com",
            affiliate_code: "39XZR7KLHECZ",
            success: true,
            bonus_amount: 1,
            created_at: "2026-04-26T23:14:01Z",
          },
        ],
        total: 2,
        page: 1,
        page_size: 10,
      }),
    );

    const page = await fetchAffiliateLogs("at-1", 1, 10);

    expect(String(fetchMock.mock.calls[0][0])).toContain("/user/aff/logs?page=1&page_size=10");
    expect(page.total).toBe(2);
    expect(page.items[0]).toEqual({
      id: 1,
      inviteeEmail: "m***@g***.com",
      affiliateCode: "39XZR7KLHECZ",
      success: true,
      failureMessage: "",
      bonusAmount: 1,
      createdAt: "2026-04-26T23:14:01Z",
    });
  });
});

describe("POST /user/aff/transfer", () => {
  it("返回划转金额与新余额", async () => {
    fetchMock.mockResolvedValue(envelope({ transferred_quota: 5.5, balance: 18 }));

    const result = await transferAffiliateQuota("at-1");

    expect(fetchMock.mock.calls[0][1]?.method).toBe("POST");
    expect(result).toEqual({ transferredQuota: 5.5, balance: 18 });
  });

  it("无可划额度时映射为稳定码 AFFILIATE_QUOTA_EMPTY", async () => {
    fetchMock.mockResolvedValue(failure(400, "AFFILIATE_QUOTA_EMPTY"));

    await expect(transferAffiliateQuota("at-1")).rejects.toMatchObject({
      code: "AFFILIATE_QUOTA_EMPTY",
    });
    await expect(transferAffiliateQuota("at-1")).rejects.toBeInstanceOf(LumioApiError);
  });
});

describe("register 的 aff 归因码", () => {
  it("携带 aff_code；未提供时不带该键", async () => {
    fetchMock.mockResolvedValue(
      envelope({ access_token: "at", refresh_token: "rt", expires_in: 3600, user: {} }),
    );

    await register({ email: "a@b.com", password: "pw123456", affCode: "abc123" });
    await register({ email: "a@b.com", password: "pw123456" });

    const withCode = JSON.parse(String(fetchMock.mock.calls[0][1]?.body));
    const withoutCode = JSON.parse(String(fetchMock.mock.calls[1][1]?.body));
    expect(withCode.aff_code).toBe("abc123");
    expect("aff_code" in withoutCode).toBe(false);
  });
});
