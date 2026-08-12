/**
 * 三站共用的域名 / 接口配置。
 *
 * 所有跨站地址只在这里定义一次：门户、两个产品站、Sub2API 与充值页。
 * 一律读环境变量再回落到生产默认值，本地联调只需改 `.env`，代码里不出现第二处硬编码。
 */

export type SiteId = "portal" | "cc" | "codex";

const DEFAULT_ROOT_DOMAIN = "lumiogame.com";
const DEFAULT_API_BASE_URL = "https://api.lumio.games";

function env(key: string): string | undefined {
  const value = (import.meta.env as Record<string, string | undefined>)[key];
  return value && value.trim() ? value.trim() : undefined;
}

function trimSlash(value: string): string {
  return value.replace(/\/+$/, "");
}

/** 会话 Cookie 与回跳白名单共用的根域。 */
export function rootDomain(): string {
  return env("VITE_ROOT_DOMAIN") ?? DEFAULT_ROOT_DOMAIN;
}

export function siteUrl(site: SiteId): string {
  const overrides: Record<SiteId, string | undefined> = {
    portal: env("VITE_PORTAL_URL"),
    cc: env("VITE_CC_URL"),
    codex: env("VITE_CODEX_URL"),
  };
  const override = overrides[site];
  if (override) return trimSlash(override);

  const root = rootDomain();
  return site === "portal" ? `https://${root}` : `https://${site}.${root}`;
}

export function apiBaseUrl(): string {
  return trimSlash(env("VITE_API_BASE_URL") ?? DEFAULT_API_BASE_URL);
}

/** 充值走 Sub2API 托管的收银台页面，不是接口调用：浏览器直接打开。 */
export function purchaseUrl(): string {
  return `${apiBaseUrl()}/purchase`;
}

export function portalUrl(path: string, next?: string | null): string {
  const base = `${siteUrl("portal")}${path}`;
  if (!next) return base;
  return `${base}?next=${encodeURIComponent(next)}`;
}

export interface AccountLinks {
  login: string;
  signup: string;
  account: string;
}

/** 产品站不做自己的登录：账号入口一律回门户，并带上回跳地址。 */
export function portalAccountLinks(next?: string | null): AccountLinks {
  return {
    login: portalUrl("/login", next),
    signup: portalUrl("/signup", next),
    account: portalUrl("/account", next),
  };
}

/**
 * `next` 来自 URL 查询串，属于用户可控输入；只放行站内相对路径与根域下的 https 地址，
 * 否则登录成功后会被引导到任意外站（开放重定向）。
 */
export function isAllowedNext(next: string | null | undefined): boolean {
  if (!next) return false;
  if (next.startsWith("//")) return false;
  if (next.startsWith("/")) return true;

  let url: URL;
  try {
    url = new URL(next);
  } catch {
    return false;
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") return false;

  const root = rootDomain();
  const host = url.hostname;
  if (host === root || host.endsWith(`.${root}`)) return true;

  // 本地联调时三站各占一个端口，主机名退化为 localhost。
  return url.protocol === "http:" && (host === "localhost" || host === "127.0.0.1");
}

export function resolveNext(next: string | null | undefined, fallback: string): string {
  return isAllowedNext(next) ? (next as string) : fallback;
}

/**
 * 生产下写父域 Cookie 让三站共享会话；开发环境（localhost / IP）不写 Domain，
 * 浏览器会拒绝为这类主机设置父域 Cookie。
 */
export function cookieDomainFor(hostname: string): string | undefined {
  const root = rootDomain();
  if (hostname === root || hostname.endsWith(`.${root}`)) return `.${root}`;
  return undefined;
}
