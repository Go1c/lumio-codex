import { api } from "@/lib/api";

import type {
  ApproveResult,
  AuthorizeContext,
  CheckoutResult,
  DeletionResult,
  Entitlement,
  ForgotPasswordResult,
  InviteAttribution,
  InviteLanding,
  MessageResult,
  OrderView,
  Plan,
  PublicConfig,
  ReferralOverview,
  RegisterResult,
  ResendResult,
  ResetTokenInspection,
  SessionSnapshot,
  SessionView,
  UserView,
} from "./types";

/** 公开配置：价格、邀请奖励天数、试用时长、下载版本。页面一律读这里，不写死。 */
export const getPublicConfig = (signal?: AbortSignal) =>
  api.get<PublicConfig>("/config/public", { signal });

// —— 注册 / 验证 / 登录 / 找回 ——

export const register = (email: string, password: string, utmSource?: string) =>
  api.post<RegisterResult>("/auth/register", {
    email,
    password,
    ...(utmSource ? { utm_source: utmSource } : {}),
  });

export const verifyEmail = (email: string, code: string) =>
  api.post<SessionSnapshot>("/auth/verify-email", { email, code });

export const resendVerificationCode = (email: string) =>
  api.post<ResendResult>("/auth/verification-code/resend", { email });

export const login = (email: string, password: string) =>
  api.post<SessionSnapshot>("/auth/login", { email, password });

export const forgotPassword = (email: string) =>
  api.post<ForgotPasswordResult>("/auth/password/forgot", { email });

export const inspectResetToken = (token: string, signal?: AbortSignal) =>
  api.get<ResetTokenInspection>(`/auth/password/reset/${encodeURIComponent(token)}`, { signal });

export const resetPassword = (token: string, password: string) =>
  api.post<MessageResult>("/auth/password/reset", { token, password });

export const logout = () => api.post<void>("/auth/logout");

export const getSession = (signal?: AbortSignal) =>
  api.get<SessionSnapshot>("/auth/session", { signal });

// —— 邀请（公开） ——

/** 服务端在此接口下发 `cch_ref` 归因 cookie（仅当 valid 为 true）。 */
export const getInviteLanding = (code: string, signal?: AbortSignal) =>
  api.get<InviteLanding>(`/invites/${encodeURIComponent(code)}`, { signal });

/**
 * 邀请横幅的权威数据源：服务端读 `cch_ref` cookie 判断当前浏览器是否仍有有效归因。
 * 只读——不下发 cookie、不记 `referral_visits`，但每次调用都会打几条查询，只在挂载时问一次。
 */
export const getCurrentInvite = (signal?: AbortSignal) =>
  api.get<InviteAttribution>("/invites/current", { signal });

// —— 账户 ——

export const getMe = (signal?: AbortSignal) => api.get<SessionSnapshot>("/me/", { signal });

export const getEntitlement = (signal?: AbortSignal) =>
  api.get<Entitlement>("/me/entitlement", { signal });

export const updateProfile = (displayName: string) =>
  api.patch<UserView>("/me/", { display_name: displayName });

export const changePassword = (currentPassword: string, newPassword: string) =>
  api.post<MessageResult>("/me/password", {
    current_password: currentPassword,
    new_password: newPassword,
  });

export const requestEmailChange = (newEmail: string) =>
  api.post<{ sent: boolean; dev_code?: string }>("/me/email-change", { new_email: newEmail });

export const confirmEmailChange = (code: string) =>
  api.post<UserView>("/me/email-change/verify", { code });

export const cancelEmailChange = () => api.delete<void>("/me/email-change");

export const listSessions = (signal?: AbortSignal) =>
  api.get<{ items: SessionView[] }>("/me/sessions", { signal });

export const revokeSession = (id: string) =>
  api.delete<void>(`/me/sessions/${encodeURIComponent(id)}`);

export const revokeOtherSessions = () =>
  api.post<{ revoked: number }>("/me/sessions/revoke-others");

export const getReferrals = (signal?: AbortSignal) =>
  api.get<ReferralOverview>("/me/referrals", { signal });

export const requestDeletion = () => api.post<DeletionResult>("/me/deletion");

export const cancelDeletion = () => api.delete<void>("/me/deletion");

// —— 订阅与付款（只在官网） ——

export const getPlan = (signal?: AbortSignal) => api.get<Plan>("/billing/plan", { signal });

/** 返回支付服务商托管页地址，站内不收集任何卡号。 */
export const checkout = (channel: string, idempotencyKey?: string) =>
  api.post<CheckoutResult>("/billing/checkout", {
    channel,
    ...(idempotencyKey ? { idempotency_key: idempotencyKey } : {}),
  });

export const listOrders = (signal?: AbortSignal) =>
  api.get<{ items: OrderView[] }>("/billing/orders", { signal });

// —— OAuth 授权页 ——

export const getAuthorizeContext = (query: URLSearchParams, signal?: AbortSignal) =>
  api.get<AuthorizeContext>("/oauth/authorize/context", { query, signal });

export const approveAuthorization = (
  query: URLSearchParams,
  body?: { device_name?: string; os_version?: string; arch?: string; app_version?: string },
) => api.post<ApproveResult>("/oauth/authorize", body ?? {}, { query });
