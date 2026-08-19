import { http, HttpResponse } from "msw";
import {
  canEditOpsConfig,
  canExportOrders,
  canManageUsers,
  canRefundOrder,
  canViewUserDetail,
} from "../api/types";
import type { AdminOrder, AdminUser, OpsConfig, SubState } from "../api/types";
import { MOCK_CREDENTIALS, MOCK_TOTP_CODE, mockState } from "./data";

const BASE = "/api/admin/v1";

/** 成功响应信封：{"data": ...}。 */
const ok = <T>(data: T, status = 200) => HttpResponse.json({ data }, { status });

/**
 * 失败响应信封：{"error":{"code","message","details"}}。
 * message 逐字取自 internal/i18n/i18n.go，前端直接展示。
 */
const fail = (status: number, code: string, message: string, details?: Record<string, unknown>) =>
  HttpResponse.json({ error: { code, message, details } }, { status });

/** 与 requireAdmin 中间件等价：半会话访问业务接口一律 401 mfa_required。 */
function guard(): Response | null {
  const { loggedIn, mfaPassed } = mockState.session;
  if (!loggedIn) return fail(401, "unauthorized", "请先登录。");
  if (!mfaPassed) return fail(401, "mfa_required", "请输入两步验证码。");
  return null;
}

function isToday(value: string | null | undefined): boolean {
  if (!value) return false;
  const date = new Date(value);
  const now = new Date();
  return (
    date.getFullYear() === now.getFullYear() &&
    date.getMonth() === now.getMonth() &&
    date.getDate() === now.getDate()
  );
}

function todayTotals(): { count: number; amount_cents: number } {
  const paidToday = mockState.orders.filter((o) => o.status === "paid" && isToday(o.paid_at));
  return {
    count: paidToday.length,
    amount_cents: paidToday.reduce((sum, order) => sum + order.amount_cents, 0),
  };
}

function pushAudit(
  action: string,
  targetType: string,
  targetID: string,
  before: unknown,
  after: unknown,
) {
  const nextID = mockState.audit.reduce((max, record) => Math.max(max, record.id), 0) + 1;
  mockState.audit.unshift({
    id: nextID,
    actor_type: "admin",
    actor_id: String(mockState.admin.id),
    action,
    target_type: targetType,
    target_id: targetID,
    before,
    after,
    ip: "127.0.0.1",
    created_at: new Date().toISOString(),
  });
}

/**
 * 与 service 层的能力谓词等价：无权时**先写 `{action}_denied` 审计再回 403**，
 * 顺序和字段都照抄后端，前端才能在 mock 上验证「越权尝试留痕」这条。
 */
function deny(
  allowed: boolean,
  action: string,
  targetType: string,
  targetID: string,
): Response | null {
  if (allowed) return null;

  pushAudit(`${action}_denied`, targetType, targetID, null, {
    actor_role: mockState.admin.role,
  });
  return fail(403, "forbidden", "没有访问权限。");
}

/** 只接受纯数字主键，与后端 strconv.ParseInt 的严格程度一致。 */
function parseUserID(raw: string | readonly string[] | undefined): number | null {
  if (typeof raw !== "string" || !/^\d+$/.test(raw)) return null;
  return Number(raw);
}

function paginate<T>(items: T[], url: URL, defaultSize = 20) {
  const page = Number(url.searchParams.get("page") ?? 1);
  const pageSize = Number(url.searchParams.get("page_size") ?? defaultSize);
  const start = (page - 1) * pageSize;
  return { slice: items.slice(start, start + pageSize), page, pageSize, total: items.length };
}

export const handlers = [
  // —— 认证 ——

  http.post(`${BASE}/auth/login`, async ({ request }) => {
    const body = (await request.json()) as { email?: string; password?: string };
    if (body.email !== MOCK_CREDENTIALS.email || body.password !== MOCK_CREDENTIALS.password) {
      return fail(401, "invalid_credentials", "邮箱或密码不正确。");
    }

    const { totpEnabled } = mockState.session;
    mockState.session = { loggedIn: true, mfaPassed: !totpEnabled, totpEnabled };
    return ok({ mfa_required: totpEnabled, mfa_enrolled: totpEnabled });
  }),

  http.post(`${BASE}/auth/login/totp`, async ({ request }) => {
    if (!mockState.session.loggedIn) return fail(401, "unauthorized", "请先登录。");

    const body = (await request.json()) as { code?: string };
    if (body.code !== MOCK_TOTP_CODE) return fail(401, "mfa_invalid", "两步验证码不正确。");

    mockState.session.mfaPassed = true;
    return ok({ mfa_passed: true });
  }),

  http.post(`${BASE}/auth/logout`, () => {
    const denied = guard();
    if (denied) return denied;

    mockState.session = { ...mockState.session, loggedIn: false, mfaPassed: false };
    return new HttpResponse(null, { status: 204 });
  }),

  http.get(`${BASE}/auth/me`, () => {
    const denied = guard();
    if (denied) return denied;

    return ok({ ...mockState.admin, totp_enabled: mockState.session.totpEnabled });
  }),

  http.post(`${BASE}/auth/totp/setup`, () => {
    const denied = guard();
    if (denied) return denied;

    const secret = "JBSWY3DPEHPK3PXP";
    const issuer = encodeURIComponent("CCHaven Admin");
    return ok({
      secret,
      uri: `otpauth://totp/${issuer}:${mockState.admin.email}?secret=${secret}&issuer=${issuer}`,
    });
  }),

  http.post(`${BASE}/auth/totp/enable`, async ({ request }) => {
    const denied = guard();
    if (denied) return denied;

    const body = (await request.json()) as { code?: string };
    if (body.code !== MOCK_TOTP_CODE) return fail(401, "mfa_invalid", "两步验证码不正确。");

    mockState.session.totpEnabled = true;
    mockState.session.mfaPassed = true;
    return ok({ totp_enabled: true });
  }),

  // —— 指标 ——

  http.get(`${BASE}/metrics/overview`, () => {
    const denied = guard();
    if (denied) return denied;

    const today = todayTotals();
    return ok({
      ...mockState.overview,
      revenue: { value: today.amount_cents, secondary: today.count },
      generated_at: new Date().toISOString(),
    });
  }),

  http.get(`${BASE}/metrics/dau`, ({ request }) => {
    const denied = guard();
    if (denied) return denied;

    const days = Number(new URL(request.url).searchParams.get("days") ?? 7);
    return ok({ items: mockState.dau.slice(-days) });
  }),

  http.get(`${BASE}/metrics/distributions`, () => {
    const denied = guard();
    if (denied) return denied;

    return ok(mockState.distributions);
  }),

  // —— 用户 ——

  http.get(`${BASE}/users`, ({ request }) => {
    const denied = guard();
    if (denied) return denied;

    const url = new URL(request.url);
    const query = (url.searchParams.get("query") ?? "").trim().toLowerCase();
    const status = url.searchParams.get("status") ?? "all";

    const filtered = mockState.users.filter((user) => {
      if (status !== "all" && status !== "" && user.sub_state !== (status as SubState)) return false;
      if (!query) return true;
      // 后端按真实邮箱 ILIKE 与 id::text 精确匹配；mock 用打码邮箱近似。
      return (
        user.email_masked.toLowerCase().includes(query) ||
        user.id.toLowerCase() === query ||
        String(user.user_id) === query
      );
    });

    const { slice, page, pageSize, total } = paginate(filtered, url);
    return ok({ items: slice, total, page, page_size: pageSize });
  }),

  /**
   * 用户详情。路径参数是数字主键，明文邮箱只在这里出现。
   * 与后端一致：owner/ops 放行，support 一律 403，两条路径都留审计。
   */
  http.get(`${BASE}/users/:id`, ({ params }) => {
    const denied = guard();
    if (denied) return denied;

    const userID = parseUserID(params.id);
    if (userID === null) return fail(400, "invalid_params", "请求参数不正确。");

    const forbidden = deny(
      canViewUserDetail(mockState.admin.role),
      "user.view_detail",
      "user",
      String(userID),
    );
    if (forbidden) return forbidden;

    const user = mockState.users.find((u) => u.user_id === userID);
    const extra = mockState.userExtras[userID];
    if (!user || !extra) return fail(404, "not_found", "资源不存在。");

    pushAudit("user.view_detail", "user", String(userID), null, null);
    return ok({
      user: {
        id: user.id,
        user_id: user.user_id,
        email: extra.email,
        display_name: extra.display_name,
        status: extra.status,
        created_at: user.created_at,
        source: user.source,
        inviter_id: user.inviter_id,
        last_active_at: user.last_active_at,
        deletion_requested_at: extra.deletion_requested_at,
      },
      entitlement: extra.entitlement,
      devices: extra.devices,
      referral: extra.referral,
      // 后端按 user_id 关联最近 10 笔；mock 里订单只带打码邮箱，用它作连接键。
      orders: mockState.orders.filter((o) => o.email_masked === user.email_masked).slice(0, 10),
    });
  }),

  http.post(`${BASE}/users/:id/:action`, ({ params }) => {
    const denied = guard();
    if (denied) return denied;

    // 后端是 strconv.ParseInt，收到展示号 U-100986 会直接 400。
    // mock 也照此严格处理，否则前端把展示号当主键用的回归不会被发现。
    const userID = parseUserID(params.id);
    if (userID === null) return fail(400, "invalid_params", "请求参数不正确。");

    const disabled = params.action === "disable";
    const forbidden = deny(
      canManageUsers(mockState.admin.role),
      disabled ? "user.disable" : "user.enable",
      "user",
      String(userID),
    );
    if (forbidden) return forbidden;

    const user = mockState.users.find((u) => u.user_id === userID);
    if (!user) return fail(404, "not_found", "资源不存在。");

    const before = user.sub_state;
    // 与后端一致：disabled 优先展示；解禁后回落到订阅状态（mock 简化为未订阅）。
    user.sub_state = disabled ? "banned" : ("none" as SubState);
    const extra = mockState.userExtras[userID];
    if (extra) extra.status = disabled ? "disabled" : "active";
    pushAudit(
      disabled ? "user.disable" : "user.enable",
      "user",
      String(userID),
      { status: before === "banned" ? "disabled" : "active" },
      { status: disabled ? "disabled" : "active", reason: "" },
    );
    return ok({ disabled });
  }),

  // —— 订单 ——

  http.get(`${BASE}/orders`, ({ request }) => {
    const denied = guard();
    if (denied) return denied;

    const url = new URL(request.url);
    const status = url.searchParams.get("status") ?? "all";
    const filtered = mockState.orders.filter(
      (order) => status === "all" || status === "" || order.status === status,
    );

    const { slice, page, pageSize, total } = paginate(filtered, url);
    return ok({ items: slice, total, page, page_size: pageSize, today: todayTotals() });
  }),

  http.get(`${BASE}/orders/export`, ({ request }) => {
    const denied = guard();
    if (denied) return denied;

    const status = new URL(request.url).searchParams.get("status") ?? "all";
    // 导出的审计目标是筛选条件而不是某一笔订单，与后端一致（all 归一化为空串）。
    const forbidden = deny(
      canExportOrders(mockState.admin.role),
      "orders.export",
      "orders",
      status === "all" ? "" : status,
    );
    if (forbidden) return forbidden;

    const rows = mockState.orders.filter(
      (order) => status === "all" || status === "" || order.status === status,
    );

    const header = "订单号,用户邮箱,金额,币种,支付渠道,状态,支付时间";
    const body = rows
      .map((order: AdminOrder) =>
        [
          order.order_no,
          order.email_masked,
          (order.amount_cents / 100).toFixed(2),
          order.currency,
          order.channel,
          order.status,
          order.paid_at ?? "",
        ].join(","),
      )
      .join("\n");

    return new HttpResponse(`\uFEFF${header}\n${body}`, {
      headers: {
        "Content-Type": "text/csv; charset=utf-8",
        "Content-Disposition": 'attachment; filename="orders.csv"',
      },
    });
  }),

  http.post(`${BASE}/orders/:orderNo/refund`, ({ params }) => {
    const denied = guard();
    if (denied) return denied;

    const forbidden = deny(
      canRefundOrder(mockState.admin.role),
      "order.refund",
      "order",
      String(params.orderNo),
    );
    if (forbidden) return forbidden;

    const order = mockState.orders.find((o) => o.order_no === params.orderNo);
    if (!order) return fail(404, "not_found", "资源不存在。");
    if (order.status !== "paid" || order.channel === "balance") {
      return fail(409, "order_not_refundable", "该订单当前状态不支持退款。");
    }

    // mock 支付通道即时成功，与 payments.MockProvider 行为一致：paid → refunded。
    order.status = "refunded";
    pushAudit("order.refund", "order", order.order_no, { status: "paid" }, { status: "refunded", reason: "" });
    return ok({ status: "refunded" });
  }),

  // —— 运营配置与审计 ——

  http.get(`${BASE}/configs`, () => {
    const denied = guard();
    if (denied) return denied;

    return ok(mockState.config);
  }),

  http.put(`${BASE}/configs`, async ({ request }) => {
    const denied = guard();
    if (denied) return denied;

    const values = (await request.json()) as Record<string, unknown>;
    if (!values || Object.keys(values).length === 0) {
      return fail(400, "invalid_params", "请求参数不正确。");
    }

    // 被拒时整批都没写成，只留一条审计，target 是排序后的 key 列表（与后端一致）。
    const forbidden = deny(
      canEditOpsConfig(mockState.admin.role),
      "ops_config.update",
      "ops_config",
      Object.keys(values).sort().join(","),
    );
    if (forbidden) return forbidden;

    const next: OpsConfig = { ...mockState.config };
    for (const [key, value] of Object.entries(values)) {
      const before =
        key === "invite.reward_days"
          ? next.invite_reward_days
          : key === "invite.trial_days"
            ? next.invite_trial_days
            : next.pricing_monthly;

      if (key === "invite.reward_days") next.invite_reward_days = Number(value);
      if (key === "invite.trial_days") next.invite_trial_days = Number(value);
      if (key === "pricing.monthly") next.pricing_monthly = value as OpsConfig["pricing_monthly"];

      pushAudit("ops_config.update", "ops_config", key, { value: before }, { value });
    }

    mockState.config = next;
    return ok(next);
  }),

  http.get(`${BASE}/audit-logs`, ({ request }) => {
    const denied = guard();
    if (denied) return denied;

    const url = new URL(request.url);
    // 空串表示不筛选，两个条件可组合（store.ListAuditLogs）。
    const actor = url.searchParams.get("actor") ?? "";
    const action = url.searchParams.get("action") ?? "";
    const filtered = mockState.audit.filter(
      (record) =>
        (actor === "" || record.actor_id === actor) && (action === "" || record.action === action),
    );

    const { slice, page, pageSize, total } = paginate(filtered, url, 50);
    return ok({ items: slice, total, page, page_size: pageSize });
  }),
];

/** 供测试直接构造用户列表用。 */
export type { AdminUser };
