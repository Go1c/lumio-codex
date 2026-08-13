/**
 * 控制面 API 的响应类型，逐字段对齐 `services/cchaven-control/internal/`：
 * - `service.PublicConfig` / `store.Release`
 * - `service.UserView` / `domain.Entitlement`
 * - `service.ReferralOverview` / `service.InviteLanding` / `service.SessionView`
 * - `service.AuthorizeContext` / `service.ApproveResult`
 * - `service.Plan` / `service.CheckoutResult` / `service.OrderView`
 *
 * 成功响应统一包在 `{"data": ...}` 里（见 internal/httpx/httpx.go），解包在 lib/api.ts。
 */

export interface PublicConfig {
  pricing: {
    amount_cents: number;
    currency: string;
    period_unit: string;
  };
  invite: {
    /** 为 0 时前端隐藏「订阅延长 X 天」相关文案。 */
    reward_days: number;
    trial_days: number;
    reward_enabled: boolean;
  };
  releases: Release[];
}

export interface Release {
  version: string;
  arch: "arm64" | "x86_64" | string;
  download_url: string;
  min_os: string;
  released_at: string;
}

export interface UserView {
  id: string;
  email: string;
  display_name: string;
  created_at: string;
  deletion_requested_at?: string;
  deletion_effective_at?: string;
}

export type EntitlementStatus = "none" | "trialing" | "active" | "expired";

export interface Entitlement {
  status: EntitlementStatus;
  kind?: string;
  expires_at?: string;
  days_left: number;
  bonus_days_total: number;
  expiring_soon: boolean;
}

export interface SessionSnapshot {
  user: UserView;
  entitlement: Entitlement;
}

export interface RegisterResult {
  email: string;
  next: string;
  /** 仅非生产环境回传，便于本地联调。 */
  dev_code?: string;
}

export interface ResendResult {
  retry_after_seconds: number;
  dev_code?: string;
}

export interface ForgotPasswordResult {
  /** 服务端下发的 6.2 节防枚举回执，前端原样展示。 */
  message: string;
  dev_token?: string;
}

export interface ResetTokenInspection {
  valid: boolean;
  email_masked: string;
}

export interface MessageResult {
  message: string;
}

export interface InviteLanding {
  valid: boolean;
  code: string;
  inviter?: string;
  trial_days: number;
}

/**
 * `GET /api/v1/invites/current`（service.InviteAttribution）：服务端读 HttpOnly 的
 * `cch_ref` cookie 裁决当前浏览器是否仍处于有效归因下。未归因时 inviter / trial_days
 * 不下发（omitempty）——此时没有任何可承诺的东西。
 */
export interface InviteAttribution {
  attributed: boolean;
  inviter?: string;
  trial_days?: number;
}

export interface ReferralItem {
  email_masked: string;
  status: "registered" | "activated";
  bonus_days: number;
  at: string;
}

export interface ReferralOverview {
  code: string;
  link: string;
  reward_days: number;
  trial_days: number;
  invited_count: number;
  total_bonus_days: number;
  items: ReferralItem[];
}

export interface SessionView {
  id: string;
  device_name: string;
  kind: "web" | "app";
  platform_detail: string;
  app_version?: string;
  last_seen_at: string;
  ip_region: string;
  current: boolean;
}

export interface Plan {
  name: string;
  amount_cents: number;
  currency: string;
  period_unit: string;
  channels: string[];
}

export interface CheckoutResult {
  order_no: string;
  pay_url: string;
  amount_cents: number;
  currency: string;
  expires_at: string;
}

export interface OrderView {
  order_no: string;
  amount_cents: number;
  currency: string;
  channel: string;
  status: string;
  paid_at?: string;
  created_at: string;
}

export interface ScopeItem {
  id: string;
  label: string;
}

export interface AuthorizeContext {
  client_name: string;
  scopes: ScopeItem[];
  redirect_kind: "loopback" | "scheme" | string;
  logged_in: boolean;
  email?: string;
}

export interface ApproveResult {
  code: string;
  redirect_to: string;
  expires_in: number;
}

export interface DeletionResult {
  effective_at: string;
}
