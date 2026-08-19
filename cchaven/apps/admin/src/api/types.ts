// 本文件的类型逐字段对齐控制面 Go 结构体，改动前请先看：
//   services/cchaven-control/internal/service/admin.go
//   services/cchaven-control/internal/api/handler_admin.go
//   services/cchaven-control/internal/store/{opsconfig,metrics,telemetry}.go

/** 管理员身份，来自 GET /auth/me。 */
export interface AdminMe {
  id: number;
  email: string;
  display_name: string;
  role: string;
  totp_enabled: boolean;
}

/** POST /auth/login 的结果。会话令牌只进 HttpOnly cookie，不回传给 JS。 */
export interface AdminLoginResult {
  mfa_required: boolean;
  mfa_enrolled: boolean;
}

/** POST /auth/totp/setup 返回的注册信息，uri 是 otpauth:// 链接。 */
export interface TotpEnrollment {
  secret: string;
  uri: string;
}

/**
 * 仪表盘上的一张指标卡。
 *
 * value 为 null 表示缺数，必须显示「—」而不是 0；delta / secondary 同理。
 */
export interface MetricCard {
  value: number | null;
  delta?: number | null;
  secondary?: number | null;
  secondary_b?: number | null;
}

export interface MetricsOverview {
  dau: MetricCard;
  signups: MetricCard;
  subscribers: MetricCard;
  revenue: MetricCard;
  trial_conversion: MetricCard;
  retention_d7: MetricCard;
  generated_at: string;
}

export interface DailyCount {
  day: string;
  count: number;
}

export interface DauSeries {
  items: DailyCount[];
}

export interface Bucket {
  label: string;
  count: number;
}

export interface Distributions {
  platform: Bucket[];
  app_version: Bucket[];
  source: Bucket[];
}

/** 订阅状态与后台筛选 chips 一一对应；disabled 优先展示为 banned。 */
export type SubState = "sub" | "trial" | "none" | "banned";
export type UserStatusFilter = "all" | SubState;

export interface AdminUser {
  /** 展示用的注册号，形如 U-100986。只用于显示，不要从中反解主键。 */
  id: string;
  /** 数字主键。详情、禁用、解禁等所有接口调用都用它。 */
  user_id: number;
  email_masked: string;
  created_at: string;
  /** 后端已本地化：自然流量 / 邀请 / 其他渠道。 */
  source: string;
  inviter_id?: string;
  /** 形如「macOS 15 · Apple Silicon」；未登录过 APP 时为空串。 */
  platform: string;
  sub_state: SubState;
  last_active_at?: string | null;
}

export interface Page<T> {
  items: T[] | null;
  total: number;
  page: number;
  page_size: number;
}

/** 管理员角色。support 是只读角色，owner 与 ops 当前能力相同。 */
export type AdminRole = "owner" | "ops" | "support";

/**
 * 权限矩阵在前端的副本，与 `internal/service/admin.go` 的 `roleCapabilities` 一一对应：
 *
 * | 能力 | support | ops | owner |
 * | --- | --- | --- | --- |
 * | 指标 / 用户列表 / 订单列表 / 审计日志 / 读运营配置 | ✅ | ✅ | ✅ |
 * | 用户详情（明文邮箱）、禁用解禁、退款、改配置、导出 CSV | ❌ | ✅ | ✅ |
 *
 * 这份副本只用来把无权的入口提前置为禁用态并说明原因，**不是**权限裁决：
 * 真正的判定永远在后端，直接敲 URL 或改这里的返回值都会被 403 挡下。
 * owner 与 ops 暂不区分是刻意的，理由见后端同名注释。
 */
const hasElevatedRole = (role: string): boolean => role === "owner" || role === "ops";

/** 能否查看用户详情（含明文邮箱）。 */
export const canViewUserDetail = (role: string): boolean => hasElevatedRole(role);

/** 能否禁用 / 解禁用户。 */
export const canManageUsers = (role: string): boolean => hasElevatedRole(role);

/** 能否发起退款。 */
export const canRefundOrder = (role: string): boolean => hasElevatedRole(role);

/** 能否修改运营配置。只读角色仍可查看当前配置。 */
export const canEditOpsConfig = (role: string): boolean => hasElevatedRole(role);

/** 能否导出订单 CSV。批量数据外带，与写操作同级。 */
export const canExportOrders = (role: string): boolean => hasElevatedRole(role);

export type EntitlementStatus = "none" | "trialing" | "active" | "expired";

export interface Entitlement {
  status: EntitlementStatus;
  kind?: string;
  expires_at?: string | null;
  days_left: number;
  bonus_days_total: number;
  /** 剩余 ≤3 天，对应 APP 侧的到期提醒阈值。 */
  expiring_soon: boolean;
}

/** 详情页头部的账号信息。邮箱在此为明文，故整个详情受二次权限保护。 */
export interface AdminUserProfile {
  id: string;
  user_id: number;
  email: string;
  display_name: string;
  status: string;
  created_at: string;
  source: string;
  inviter_id?: string;
  last_active_at?: string | null;
  deletion_requested_at?: string | null;
}

export interface AdminDevice {
  device_id: string;
  /** 后端已拼好，如「macOS 15 · Apple Silicon」。 */
  platform: string;
  app_version?: string;
  first_seen_at: string;
  last_seen_at: string;
}

/** 邀请进度里的被邀请者邮箱仍然打码。 */
export interface ReferralItem {
  email_masked: string;
  status: string;
  bonus_days: number;
  at: string;
}

export interface AdminReferralView {
  invited_count: number;
  total_bonus_days: number;
  items: ReferralItem[] | null;
}

export interface AdminUserDetail {
  user: AdminUserProfile;
  entitlement: Entitlement;
  devices: AdminDevice[] | null;
  referral: AdminReferralView;
  /** 最近 10 笔，行结构与订单列表一致。 */
  orders: AdminOrder[] | null;
}

export type OrderStatus = "pending" | "paid" | "refunding" | "refunded" | "failed";
export type OrderStatusFilter = "all" | "paid" | "refunding" | "refunded" | "failed";
export type PaymentChannel = "alipay" | "wechat" | "card" | "mock" | "balance";

export interface AdminOrder {
  /** CC{YYYYMMDD}-{6 位序号} */
  order_no: string;
  email_masked: string;
  amount_cents: number;
  currency: string;
  channel: PaymentChannel | string;
  status: OrderStatus;
  paid_at?: string | null;
  created_at: string;
}

export interface OrderPage extends Page<AdminOrder> {
  today: { count: number; amount_cents: number };
}

export interface Price {
  amount_cents: number;
  currency: string;
}

export interface OpsConfig {
  invite_reward_days: number;
  invite_trial_days: number;
  pricing_monthly: Price;
}

/** PUT /configs 的请求体是 key→value 映射，key 用点号形式。 */
export const CONFIG_KEYS = {
  inviteRewardDays: "invite.reward_days",
  inviteTrialDays: "invite.trial_days",
  pricingMonthly: "pricing.monthly",
} as const;

export interface AuditRecord {
  id: number;
  actor_type: string;
  actor_id: string;
  action: string;
  target_type: string;
  target_id: string;
  before?: unknown;
  after?: unknown;
  ip: string;
  created_at: string;
}
