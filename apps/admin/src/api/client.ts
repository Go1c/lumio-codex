import { t } from "../i18n";

export const API_BASE = "/api/admin/v1";

/**
 * 控制面的错误响应：{"error":{"code","message","details"}}。
 * message 已是 6.2 节规范文案，直接展示，不要在前端重写。
 */
export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly details?: Record<string, unknown>;

  constructor(status: number, code: string, message: string, details?: Record<string, unknown>) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.details = details;
  }

  /** 半会话（未过两步验证）访问业务接口时后端返回的信号。 */
  get isMfaRequired(): boolean {
    return this.code === "mfa_required";
  }

  get isUnauthenticated(): boolean {
    return this.status === 401 && !this.isMfaRequired;
  }

  /** 非管理员或被停用的管理员：展示 403 页。 */
  get isForbidden(): boolean {
    return this.status === 403;
  }
}

interface RequestOptions {
  method?: string;
  body?: unknown;
  query?: Record<string, string | number | undefined>;
  signal?: AbortSignal;
}

function buildURL(path: string, query?: RequestOptions["query"]): string {
  const url = `${API_BASE}${path}`;
  if (!query) return url;

  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined && value !== "") search.set(key, String(value));
  }
  const qs = search.toString();
  return qs ? `${url}?${qs}` : url;
}

async function toApiError(response: Response): Promise<ApiError> {
  try {
    const payload = (await response.json()) as {
      error?: { code?: string; message?: string; details?: Record<string, unknown> };
    };
    const err = payload.error;
    if (err?.message) {
      return new ApiError(response.status, err.code ?? "unknown", err.message, err.details);
    }
  } catch {
    // 响应不是 JSON（如网关返回的 HTML 错误页），落到下面的兜底文案。
  }
  return new ApiError(response.status, "unknown", t("error.generic"));
}

/** request 是所有管理端调用的唯一出口：一律带 cookie，一律解开 {"data": ...} 信封。 */
export async function request<T>(path: string, options: RequestOptions = {}): Promise<T> {
  const { method = "GET", body, query, signal } = options;

  let response: Response;
  try {
    response = await fetch(buildURL(path, query), {
      method,
      // 会话走 HttpOnly cookie cch_admin，必须带上凭证。
      credentials: "include",
      headers: body === undefined ? undefined : { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal,
    });
  } catch (cause) {
    if (cause instanceof DOMException && cause.name === "AbortError") throw cause;
    throw new ApiError(0, "network_error", t("error.network"));
  }

  if (!response.ok) throw await toApiError(response);
  if (response.status === 204) return undefined as T;

  const payload = (await response.json()) as { data?: T };
  return payload.data as T;
}

/** requestBlob 用于 CSV 导出，走同一套鉴权与错误约定。 */
export async function requestBlob(path: string, query?: RequestOptions["query"]): Promise<Blob> {
  let response: Response;
  try {
    response = await fetch(buildURL(path, query), { credentials: "include" });
  } catch {
    throw new ApiError(0, "network_error", t("error.network"));
  }
  if (!response.ok) throw await toApiError(response);
  return response.blob();
}
