/**
 * CCHaven 控制面的 OAuth 授权端点客户端。
 *
 * 账号已收口到 Sub2API，控制面不再是身份提供方，但仍是 CC 桌面端的 token issuer：
 * 门户带着 Sub2API 令牌代表用户确认授权，控制面据此签发授权码。
 *
 * 它的信封与 Sub2API 不同（成功 `{data}`、失败 `{error:{code,message,details}}`，
 * 见 `cchaven/services/cchaven-control/internal/httpx/httpx.go`），因此不能复用
 * `@lumio/auth` 的客户端。`message` 已是服务端下发的 zh-CN 文案，可直接展示。
 */

import { ccControlBaseUrl } from "@lumio/ui/config";

const UNAVAILABLE = "无法连接授权服务，请稍后重试。";

export interface AuthorizeScope {
  id: string;
  label: string;
}

export interface AuthorizeContext {
  clientName: string;
  scopes: AuthorizeScope[];
  loggedIn: boolean;
  email: string;
}

export interface ApproveResult {
  code: string;
  redirectTo: string;
  expiresIn: number;
}

export class CcControlError extends Error {
  readonly code: string;

  constructor(code: string, message: string) {
    super(message);
    this.name = "CcControlError";
    this.code = code;
  }
}

type Json = Record<string, unknown>;

function str(value: unknown, fallback = ""): string {
  return typeof value === "string" ? value : fallback;
}

function num(value: unknown, fallback = 0): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function url(path: string, query: URLSearchParams): string {
  return `${ccControlBaseUrl()}/api/v1${path}?${query.toString()}`;
}

async function send(target: string, init: RequestInit): Promise<Response> {
  try {
    return await fetch(target, init);
  } catch {
    // 异常文本会带上完整地址与查询串（含 code_challenge），折叠成固定文案。
    throw new CcControlError("network", UNAVAILABLE);
  }
}

async function readData(response: Response): Promise<Json> {
  const text = await response.text().catch(() => "");
  let body: Json | null = null;
  try {
    body = JSON.parse(text) as Json;
  } catch {
    body = null;
  }

  if (!response.ok) {
    const failure = (body?.error ?? {}) as Json;
    throw new CcControlError(
      str(failure.code, "service_unavailable"),
      str(failure.message) || UNAVAILABLE,
    );
  }
  const data = body?.data;
  if (!data || typeof data !== "object") {
    throw new CcControlError("service_unavailable", UNAVAILABLE);
  }
  return data as Json;
}

function authHeaders(accessToken?: string): Record<string, string> {
  const headers: Record<string, string> = { Accept: "application/json" };
  if (accessToken) headers.Authorization = `Bearer ${accessToken}`;
  return headers;
}

/**
 * 确认页所需的「谁在请求什么权限」。未登录也返回 200，页面据此先展示再引导登录；
 * 带上令牌时控制面会回填 `logged_in` 与邮箱——那才是真正会被授权的账号。
 */
export async function fetchAuthorizeContext(
  query: URLSearchParams,
  accessToken?: string,
  signal?: AbortSignal,
): Promise<AuthorizeContext> {
  const response = await send(url("/oauth/authorize/context", query), {
    method: "GET",
    headers: authHeaders(accessToken),
    signal,
  });
  const data = await readData(response);

  return {
    clientName: str(data.client_name),
    scopes: (Array.isArray(data.scopes) ? data.scopes : []).map((item) => {
      const scope = (item ?? {}) as Json;
      return { id: str(scope.id), label: str(scope.label) || str(scope.id) };
    }),
    loggedIn: data.logged_in === true,
    email: str(data.email),
  };
}

/** 用户点「同意授权」：控制面只认 Sub2API 令牌，cookie 会话一律 401。 */
export async function approveAuthorization(
  query: URLSearchParams,
  accessToken: string,
): Promise<ApproveResult> {
  const response = await send(url("/oauth/authorize", query), {
    method: "POST",
    headers: { ...authHeaders(accessToken), "Content-Type": "application/json" },
    body: JSON.stringify({}),
  });
  const data = await readData(response);

  return {
    code: str(data.code),
    redirectTo: str(data.redirect_to),
    expiresIn: num(data.expires_in),
  };
}

export function messageOfControlError(error: unknown): string {
  return error instanceof CcControlError ? error.message : UNAVAILABLE;
}
