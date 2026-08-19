// Mock 数据。本机起不了控制面的 PostgreSQL，开发与测试都跑在这份内存数据上。
// 所有字段与 services/cchaven-control 的真实响应结构严格对齐，改后端结构时这里要同步。
import type {
  AdminDevice,
  AdminOrder,
  AdminReferralView,
  AdminUser,
  AuditRecord,
  DailyCount,
  Distributions,
  Entitlement,
  MetricsOverview,
  OpsConfig,
} from "../api/types";

export interface MockSession {
  loggedIn: boolean;
  /** 半会话：登录成功但未过两步验证，访问业务接口一律 401 mfa_required。 */
  mfaPassed: boolean;
  totpEnabled: boolean;
}

/**
 * 详情接口独有的数据。列表行不下发这些字段，所以单独放一张表，
 * 按数字主键 user_id 关联——和真实后端一样，展示号 U-100986 不参与任何查找。
 */
export interface MockUserExtra {
  /** 明文邮箱，只有详情接口返回。 */
  email: string;
  display_name: string;
  status: "active" | "disabled";
  deletion_requested_at?: string | null;
  entitlement: Entitlement;
  devices: AdminDevice[];
  referral: AdminReferralView;
}

export interface MockState {
  session: MockSession;
  admin: { id: number; email: string; display_name: string; role: string };
  users: AdminUser[];
  userExtras: Record<number, MockUserExtra>;
  orders: AdminOrder[];
  config: OpsConfig;
  audit: AuditRecord[];
  overview: MetricsOverview;
  dau: DailyCount[];
  distributions: Distributions;
}

export const MOCK_CREDENTIALS = { email: "admin@cchaven.cn", password: "admin12345" };
/** 演示用固定验证码，真实环境由 TOTP 算法校验。 */
export const MOCK_TOTP_CODE = "123456";

const DAY = 86_400_000;

function iso(offsetMs: number, base = Date.now()): string {
  return new Date(base - offsetMs).toISOString();
}

/**
 * 以本地零点为基准取「N 天前的 hh:mm」。
 * 订单的当日汇总要按自然日统计，用「此刻减去若干小时」会在深夜跨日，导致数据飘。
 */
function dayAt(daysAgo: number, hours: number, minutes: number): string {
  const date = new Date();
  date.setHours(hours, minutes, 0, 0);
  date.setDate(date.getDate() - daysAgo);
  return date.toISOString();
}

/** 订单号格式：CC{YYYYMMDD}-{6 位序号}。 */
function orderNo(daysAgo: number, seq: number): string {
  const date = new Date();
  date.setHours(12, 0, 0, 0);
  date.setDate(date.getDate() - daysAgo);
  const stamp =
    `${date.getFullYear()}` +
    `${String(date.getMonth() + 1).padStart(2, "0")}` +
    `${String(date.getDate()).padStart(2, "0")}`;
  return `CC${stamp}-${String(seq).padStart(6, "0")}`;
}

function buildUsers(): AdminUser[] {
  return [
    {
      id: "U-100986",
      user_id: 100986,
      email_masked: "m***y@example.com",
      created_at: iso(71 * DAY),
      source: "自然流量",
      platform: "macOS 15 · Apple Silicon",
      sub_state: "sub",
      last_active_at: iso(30_000),
    },
    {
      id: "U-100985",
      user_id: 100985,
      email_masked: "w***g@gmail.com",
      created_at: iso(1 * DAY),
      source: "邀请",
      inviter_id: "U-100986",
      platform: "macOS 14 · Apple Silicon",
      sub_state: "trial",
      last_active_at: iso(13 * 60_000),
    },
    {
      id: "U-100984",
      user_id: 100984,
      email_masked: "l***i@qq.com",
      created_at: iso(1 * DAY - 3600_000),
      source: "邀请",
      inviter_id: "U-100986",
      // 未登录过 APP：后端返回空串，列表显示「—（未登录 APP）」。
      platform: "",
      sub_state: "none",
      last_active_at: null,
    },
    {
      id: "U-100983",
      user_id: 100983,
      email_masked: "c***n@163.com",
      created_at: iso(2 * DAY),
      source: "自然流量",
      platform: "macOS 15 · Intel",
      sub_state: "sub",
      last_active_at: iso(2 * 3600_000),
    },
    {
      id: "U-100982",
      user_id: 100982,
      email_masked: "z***o@outlook.com",
      created_at: iso(3 * DAY),
      source: "其他渠道",
      platform: "macOS 14 · Apple Silicon",
      sub_state: "trial",
      last_active_at: iso(1.5 * DAY),
    },
    {
      id: "U-100981",
      user_id: 100981,
      email_masked: "s***n@qq.com",
      created_at: iso(4 * DAY),
      source: "自然流量",
      platform: "macOS 13 · Intel",
      sub_state: "none",
      last_active_at: iso(3 * DAY),
    },
    {
      id: "U-100980",
      user_id: 100980,
      email_masked: "s***m@tmp.io",
      created_at: iso(5 * DAY),
      source: "其他渠道",
      platform: "macOS 14 · Apple Silicon",
      sub_state: "banned",
      last_active_at: iso(5 * DAY),
    },
  ];
}

function noEntitlement(): Entitlement {
  return { status: "none", days_left: 0, bonus_days_total: 0, expiring_soon: false };
}

function activeEntitlement(kind: "trial" | "paid", daysLeft: number, bonus = 0): Entitlement {
  return {
    status: kind === "trial" ? "trialing" : "active",
    kind,
    expires_at: iso(-daysLeft * DAY),
    days_left: daysLeft,
    bonus_days_total: bonus,
    expiring_soon: daysLeft <= 3,
  };
}

function emptyReferral(): AdminReferralView {
  return { invited_count: 0, total_bonus_days: 0, items: [] };
}

function buildUserExtras(): Record<number, MockUserExtra> {
  return {
    100986: {
      email: "mary@example.com",
      display_name: "Mary",
      status: "active",
      entitlement: activeEntitlement("paid", 96, 14),
      devices: [
        {
          device_id: "D-9F2A41",
          platform: "macOS 15 · Apple Silicon",
          app_version: "1.4.2",
          first_seen_at: iso(70 * DAY),
          last_seen_at: iso(30_000),
        },
        {
          device_id: "D-31C7B0",
          platform: "macOS 14 · Intel",
          app_version: "1.4.0",
          first_seen_at: iso(60 * DAY),
          last_seen_at: iso(12 * DAY),
        },
      ],
      referral: {
        invited_count: 2,
        total_bonus_days: 14,
        items: [
          // 被邀请者是另一个用户，其邮箱不在本页授权范围内，后端也只给打码。
          { email_masked: "w***g@gmail.com", status: "activated", bonus_days: 7, at: iso(1 * DAY) },
          { email_masked: "l***i@qq.com", status: "registered", bonus_days: 0, at: iso(1 * DAY) },
        ],
      },
    },
    100985: {
      email: "wangfang@gmail.com",
      display_name: "王芳",
      status: "active",
      entitlement: activeEntitlement("trial", 29),
      devices: [
        {
          device_id: "D-6B14E8",
          platform: "macOS 14 · Apple Silicon",
          app_version: "1.4.2",
          first_seen_at: iso(1 * DAY),
          last_seen_at: iso(13 * 60_000),
        },
      ],
      referral: emptyReferral(),
    },
    // 无设备、无订单、无邀请：详情页三处 empty 态都能在这个用户上看到。
    100984: {
      email: "liuyi@qq.com",
      display_name: "",
      status: "active",
      entitlement: noEntitlement(),
      devices: [],
      referral: emptyReferral(),
    },
    100983: {
      email: "chen@163.com",
      display_name: "陈工",
      status: "active",
      entitlement: activeEntitlement("paid", 2),
      devices: [
        {
          device_id: "D-77A0C2",
          platform: "macOS 15 · Intel",
          app_version: "1.4.1",
          first_seen_at: iso(2 * DAY),
          last_seen_at: iso(2 * 3600_000),
        },
      ],
      referral: emptyReferral(),
    },
    100982: {
      email: "zhao@outlook.com",
      display_name: "赵敏",
      status: "active",
      deletion_requested_at: iso(2 * DAY),
      entitlement: activeEntitlement("trial", 12),
      devices: [
        {
          device_id: "D-2E55D9",
          platform: "macOS 14 · Apple Silicon",
          app_version: "1.4.2",
          first_seen_at: iso(3 * DAY),
          last_seen_at: iso(1.5 * DAY),
        },
      ],
      referral: emptyReferral(),
    },
    100981: {
      email: "sun@qq.com",
      display_name: "孙宁",
      status: "active",
      entitlement: noEntitlement(),
      devices: [
        {
          device_id: "D-84BB13",
          platform: "macOS 13 · Intel",
          app_version: "1.4.0",
          first_seen_at: iso(4 * DAY),
          last_seen_at: iso(3 * DAY),
        },
      ],
      referral: emptyReferral(),
    },
    100980: {
      email: "spam@tmp.io",
      display_name: "",
      status: "disabled",
      entitlement: noEntitlement(),
      devices: [],
      referral: emptyReferral(),
    },
  };
}

function buildOrders(): AdminOrder[] {
  return [
    {
      order_no: orderNo(0, 100486),
      email_masked: "m***y@example.com",
      amount_cents: 6800,
      currency: "CNY",
      channel: "alipay",
      status: "paid",
      paid_at: dayAt(0, 9, 41),
      created_at: dayAt(0, 9, 41),
    },
    {
      order_no: orderNo(0, 100485),
      email_masked: "c***n@163.com",
      amount_cents: 6800,
      currency: "CNY",
      channel: "wechat",
      status: "paid",
      paid_at: dayAt(0, 8, 17),
      created_at: dayAt(0, 8, 17),
    },
    {
      order_no: orderNo(1, 100471),
      email_masked: "z***o@outlook.com",
      amount_cents: 6800,
      currency: "CNY",
      channel: "alipay",
      status: "failed",
      paid_at: null,
      created_at: dayAt(1, 22, 3),
    },
    {
      order_no: orderNo(1, 100468),
      email_masked: "s***n@qq.com",
      amount_cents: 6800,
      currency: "CNY",
      channel: "card",
      status: "refunded",
      paid_at: dayAt(1, 16, 44),
      created_at: dayAt(1, 16, 44),
    },
    {
      order_no: orderNo(1, 100455),
      email_masked: "w***g@gmail.com",
      amount_cents: 6800,
      currency: "CNY",
      channel: "wechat",
      status: "paid",
      paid_at: dayAt(1, 10, 29),
      created_at: dayAt(1, 10, 29),
    },
    {
      order_no: orderNo(1, 100499),
      email_masked: "b***e@example.com",
      amount_cents: 1990,
      currency: "CNY",
      channel: "balance",
      status: "paid",
      paid_at: dayAt(1, 11, 0),
      created_at: dayAt(1, 11, 0),
    },
    {
      order_no: orderNo(2, 100440),
      email_masked: "c***n@163.com",
      amount_cents: 6800,
      currency: "CNY",
      channel: "alipay",
      status: "refunding",
      paid_at: dayAt(2, 19, 12),
      created_at: dayAt(2, 19, 12),
    },
  ];
}

function buildOverview(): MetricsOverview {
  return {
    dau: { value: 1284, delta: 0.058 },
    signups: { value: 96, secondary: 41 },
    subscribers: { value: 862, secondary: 214 },
    // 单位是分，卡片按元展示。
    revenue: { value: 326_400, secondary: 48 },
    // 试用队列为空：后端返回 value=null，卡片必须显示「—」而不是 0。
    trial_conversion: { value: null },
    retention_d7: { value: 0.614, delta: -0.012 },
    generated_at: new Date().toISOString(),
  };
}

function buildDau(): DailyCount[] {
  const counts = [980, 1010, 1052, 1121, 1087, 1214, 1284];
  return counts.map((count, index) => ({
    day: iso((6 - index) * DAY),
    count,
  }));
}

function buildDistributions(): Distributions {
  return {
    platform: [
      { label: "macOS · Apple Silicon", count: 780 },
      { label: "macOS · Intel", count: 220 },
    ],
    app_version: [
      { label: "1.4.2", count: 640 },
      { label: "1.4.1", count: 270 },
      { label: "1.4.0", count: 90 },
    ],
    source: [
      { label: "自然流量", count: 520 },
      { label: "好友邀请", count: 380 },
      { label: "其他渠道", count: 100 },
    ],
  };
}

function buildAudit(): AuditRecord[] {
  return [
    {
      id: 3,
      actor_type: "admin",
      actor_id: "1",
      action: "user.disable",
      target_type: "user",
      target_id: "100980",
      before: { status: "active" },
      after: { status: "disabled", reason: "垃圾注册" },
      ip: "203.0.113.9",
      created_at: iso(5 * DAY),
    },
    {
      id: 2,
      actor_type: "admin",
      actor_id: "1",
      action: "order.refund",
      target_type: "order",
      target_id: orderNo(1, 100468),
      before: { status: "paid" },
      after: { status: "refunded", reason: "用户申请" },
      ip: "203.0.113.9",
      created_at: iso(1 * DAY),
    },
    {
      id: 1,
      actor_type: "admin",
      actor_id: "1",
      action: "ops_config.update",
      target_type: "ops_config",
      target_id: "pricing.monthly",
      before: { value: '{"amount_cents":9800,"currency":"CNY"}' },
      after: { value: { amount_cents: 6800, currency: "CNY" } },
      ip: "203.0.113.9",
      created_at: iso(9 * DAY),
    },
  ];
}

export function freshState(): MockState {
  return {
    session: { loggedIn: false, mfaPassed: false, totpEnabled: false },
    admin: { id: 1, email: MOCK_CREDENTIALS.email, display_name: "运营管理员", role: "owner" },
    users: buildUsers(),
    userExtras: buildUserExtras(),
    orders: buildOrders(),
    config: {
      invite_reward_days: 7,
      invite_trial_days: 30,
      pricing_monthly: { amount_cents: 6800, currency: "CNY" },
    },
    audit: buildAudit(),
    overview: buildOverview(),
    dau: buildDau(),
    distributions: buildDistributions(),
  };
}

export let mockState: MockState = freshState();

/** 每个测试前调用，避免用例之间互相污染。 */
export function resetMockState(overrides?: Partial<MockState>): MockState {
  mockState = { ...freshState(), ...overrides };
  return mockState;
}
