import { t } from "@/i18n";

/**
 * 控制面 HTTP 客户端。
 *
 * 约定（对齐 services/cchaven-control）：
 * - 成功：`{"data": ...}`；失败：`{"error":{"code","message","details"}}` + 语义化状态码。
 * - 官网会话走 HttpOnly cookie（cch_sess / cch_refresh），所以一律 `credentials: "include"`。
 * - access token 15 分钟过期：收到 401 `session_expired` 时先 `POST /auth/refresh` 再重试一次。
 */

const API_BASE = (import.meta.env.VITE_API_BASE_URL ?? "").replace(/\/$/, "");

export const API_PREFIX = `${API_BASE}/api/v1`;

export interface ApiErrorPayload {
  code: string;
  message: string;
  details?: Record<string, unknown>;
}

/** 后端返回的业务错误。`message` 已是规范文案，可直接展示；`code` 用于交互分支。 */
export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly details: Record<string, unknown>;

  constructor(status: number, payload: ApiErrorPayload) {
    super(payload.message || t("common.unknown_error"));
    this.name = "ApiError";
    this.status = status;
    this.code = payload.code || "unknown_error";
    this.details = payload.details ?? {};
  }

  /** 限频 / 锁定场景的可重试秒数。 */
  get retryAfterSeconds(): number | undefined {
    const value = this.details.retry_after_seconds;
    return typeof value === "number" ? value : undefined;
  }

  /** 验证码剩余尝试次数。 */
  get attemptsRemaining(): number | undefined {
    const value = this.details.attempts_remaining;
    return typeof value === "number" ? value : undefined;
  }

  get reason(): string | undefined {
    const value = this.details.reason;
    return typeof value === "string" ? value : undefined;
  }
}

/** 网络不可达 / 请求被中断，与业务错误区分开，页面据此展示「检查网络后重试」。 */
export class NetworkError extends Error {
  readonly code = "network_error";

  constructor(cause?: unknown) {
    super(t("common.network_error"));
    this.name = "NetworkError";
    this.cause = cause;
  }
}

export function isApiError(error: unknown, code?: string): error is ApiError {
  return error instanceof ApiError && (code === undefined || error.code === code);
}

/** 会话失效（refresh 也救不回来）时的全局回调，由 SessionProvider 注册。 */
type SessionExpiredListener = () => void;
let sessionExpiredListener: SessionExpiredListener | null = null;

export function onSessionExpired(listener: SessionExpiredListener | null) {
  sessionExpiredListener = listener;
}

interface RequestOptions {
  method?: "GET" | "POST" | "PATCH" | "PUT" | "DELETE";
  body?: unknown;
  query?: Record<string, string | number | undefined | null> | URLSearchParams;
  signal?: AbortSignal;
  /** 内部使用：refresh 自身与重试请求不再触发二次刷新。 */
  allowRefresh?: boolean;
}

function buildURL(path: string, query?: RequestOptions["query"]): string {
  const url = `${API_PREFIX}${path}`;
  if (!query) return url;

  const params =
    query instanceof URLSearchParams
      ? query
      : new URLSearchParams(
          Object.entries(query)
            .filter(([, value]) => value !== undefined && value !== null && value !== "")
            .map(([key, value]) => [key, String(value)]),
        );

  const qs = params.toString();
  return qs ? `${url}?${qs}` : url;
}

async function parseError(response: Response): Promise<ApiError> {
  let payload: ApiErrorPayload = { code: "unknown_error", message: t("common.unknown_error") };
  try {
    const body = (await response.json()) as { error?: ApiErrorPayload };
    if (body?.error) payload = body.error;
  } catch {
    // 非 JSON 响应（网关错误页等）保持默认文案。
  }
  return new ApiError(response.status, payload);
}

async function rawRequest(path: string, options: RequestOptions): Promise<Response> {
  const { method = "GET", body, query, signal } = options;

  const headers: Record<string, string> = { Accept: "application/json" };
  if (body !== undefined) headers["Content-Type"] = "application/json";

  try {
    return await fetch(buildURL(path, query), {
      method,
      headers,
      // 写操作的 Origin 头由浏览器自动附带，服务端据此做 CSRF 校验。
      credentials: "include",
      body: body === undefined ? undefined : JSON.stringify(body),
      signal,
    });
  } catch (error) {
    if (error instanceof DOMException && error.name === "AbortError") throw error;
    throw new NetworkError(error);
  }
}

export async function request<T>(path: string, options: RequestOptions = {}): Promise<T> {
  const { allowRefresh = true } = options;

  let response = await rawRequest(path, options);

  if (response.status === 401 && allowRefresh) {
    const error = await parseError(response.clone());
    if (error.code === "session_expired") {
      const refreshed = await rawRequest("/auth/refresh", { method: "POST" })
        .then((res) => res.ok)
        .catch(() => false);

      if (refreshed) {
        response = await rawRequest(path, options);
      } else {
        sessionExpiredListener?.();
        throw error;
      }
    }
  }

  if (!response.ok) {
    const error = await parseError(response);
    if (response.status === 401) sessionExpiredListener?.();
    throw error;
  }

  if (response.status === 204) return undefined as T;

  const text = await response.text();
  if (!text) return undefined as T;

  const body = JSON.parse(text) as { data?: T };
  return body.data as T;
}

export const api = {
  get: <T>(path: string, options?: Omit<RequestOptions, "method" | "body">) =>
    request<T>(path, { ...options, method: "GET" }),
  post: <T>(path: string, body?: unknown, options?: Omit<RequestOptions, "method" | "body">) =>
    request<T>(path, { ...options, method: "POST", body }),
  patch: <T>(path: string, body?: unknown, options?: Omit<RequestOptions, "method" | "body">) =>
    request<T>(path, { ...options, method: "PATCH", body }),
  delete: <T>(path: string, options?: Omit<RequestOptions, "method" | "body">) =>
    request<T>(path, { ...options, method: "DELETE" }),
};
