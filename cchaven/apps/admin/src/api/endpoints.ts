import { request, requestBlob } from "./client";
import type {
  AdminLoginResult,
  AdminMe,
  AdminOrder,
  AdminUser,
  AdminUserDetail,
  AuditRecord,
  Distributions,
  DauSeries,
  MetricsOverview,
  OpsConfig,
  OrderPage,
  OrderStatusFilter,
  Page,
  TotpEnrollment,
  UserStatusFilter,
} from "./types";

// —— 认证 ——

export const login = (email: string, password: string) =>
  request<AdminLoginResult>("/auth/login", { method: "POST", body: { email, password } });

/** 半会话补交 TOTP，通过后会话升级为完整会话。 */
export const verifyLoginTotp = (code: string) =>
  request<{ mfa_passed: boolean }>("/auth/login/totp", { method: "POST", body: { code } });

export const logout = () => request<void>("/auth/logout", { method: "POST" });

export const fetchMe = () => request<AdminMe>("/auth/me");

export const setupTotp = () => request<TotpEnrollment>("/auth/totp/setup", { method: "POST" });

export const enableTotp = (code: string) =>
  request<{ totp_enabled: boolean }>("/auth/totp/enable", { method: "POST", body: { code } });

// —— 指标 ——

export const fetchOverview = (signal?: AbortSignal) =>
  request<MetricsOverview>("/metrics/overview", { signal });

export const fetchDau = (days = 7, signal?: AbortSignal) =>
  request<DauSeries>("/metrics/dau", { query: { days }, signal });

export const fetchDistributions = (days = 30, signal?: AbortSignal) =>
  request<Distributions>("/metrics/distributions", { query: { days }, signal });

// —— 用户 ——

export const fetchUsers = (
  params: { query?: string; status?: UserStatusFilter; page?: number; pageSize?: number },
  signal?: AbortSignal,
) =>
  request<Page<AdminUser>>("/users", {
    query: {
      query: params.query || undefined,
      status: params.status ?? "all",
      page: params.page ?? 1,
      page_size: params.pageSize ?? 20,
    },
    signal,
  });

/**
 * 详情接口的路径参数是数字主键 user_id，不是展示号 U-100986。
 * 每次成功访问都会在后端留下 user.view_detail 审计记录。
 */
export const fetchUserDetail = (userID: number, signal?: AbortSignal) =>
  request<AdminUserDetail>(`/users/${userID}`, { signal });

/** 同样只接受数字主键。展示号是后端的呈现约定，前端不做反解。 */
export const setUserDisabled = (userID: number, disabled: boolean, reason?: string) =>
  request<{ disabled: boolean }>(`/users/${userID}/${disabled ? "disable" : "enable"}`, {
    method: "POST",
    body: reason ? { reason } : {},
  });

// —— 订单 ——

export const fetchOrders = (
  params: { status?: OrderStatusFilter; page?: number; pageSize?: number },
  signal?: AbortSignal,
) =>
  request<OrderPage>("/orders", {
    query: {
      status: params.status ?? "all",
      page: params.page ?? 1,
      page_size: params.pageSize ?? 20,
    },
    signal,
  });

export const exportOrdersCSV = (status: OrderStatusFilter) =>
  requestBlob("/orders/export", { status });

/** 退款是幂等的：返回 refunding 表示渠道仍在处理，refunded 表示已完成。 */
export const refundOrder = (orderNo: string, reason?: string) =>
  request<{ status: AdminOrder["status"] }>(`/orders/${encodeURIComponent(orderNo)}/refund`, {
    method: "POST",
    body: reason ? { reason } : {},
  });

// —— 运营配置与审计 ——

export const fetchConfigs = (signal?: AbortSignal) => request<OpsConfig>("/configs", { signal });

/** 请求体是 key→value 映射，key 用点号形式（invite.reward_days 等）。 */
export const saveConfigs = (values: Record<string, unknown>) =>
  request<OpsConfig>("/configs", { method: "PUT", body: values });

/** actor 是管理员数字 ID，action 是动作枚举；空串表示不筛选，可组合。 */
export const fetchAuditLogs = (
  params: { actor?: string; action?: string; page?: number; pageSize?: number } = {},
  signal?: AbortSignal,
) =>
  request<Page<AuditRecord>>("/audit-logs", {
    query: {
      actor: params.actor || undefined,
      action: params.action || undefined,
      page: params.page ?? 1,
      page_size: params.pageSize ?? 10,
    },
    signal,
  });
