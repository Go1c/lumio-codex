/**
 * 门户与产品站共用的域名 / 接口配置。
 *
 * 所有跨站地址只在这里定义一次：门户、产品站路由、Sub2API 与充值页。
 * 一律读环境变量再回落到生产默认值，本地联调只需改 `.env`，代码里不出现第二处硬编码。
 */

export type SiteId = "portal" | "cc" | "codex";

const DEFAULT_ROOT_DOMAIN = "bestcodex.app";
const DEFAULT_API_BASE_URL = "https://api.lumio.games";
const DEFAULT_CC_CONTROL_BASE_URL = "https://api.cc.bestcodex.app";
/** Lumio 飞书客服群；换群改这里或覆盖环境变量。 */
const DEFAULT_SUPPORT_FEISHU_URL =
  "https://applink.feishu.cn/client/chat/chatter/add_by_link?link_token=802t132e-f554-4ec2-9b18-5f83276fcb9f";
const DEFAULT_SUPPORT_QQ_NUMBER = "1073671738";
/** 显式写 `off` 可关掉一条已有默认值的通道。 */
const CHANNEL_OFF = "off";

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

/** 仍在线上的旧门户主机。Cookie 根域是 bestcodex.app，这里写的是 host-only，不能共享。 */
const LEGACY_ACCOUNT_HOSTS = ["lumiogame.com", "www.lumiogame.com"];

export function isLocalHostname(hostname: string): boolean {
  return hostname === "localhost" || hostname === "127.0.0.1";
}

/** 规范账号 origin：门户默认就是营销 apex，和产品站共享 `.bestcodex.app` Cookie。 */
export function canonicalAccountOrigin(): string {
  try {
    return new URL(siteUrl("portal")).origin;
  } catch {
    return `https://${rootDomain()}`;
  }
}

export function isOfficialAccountHost(hostname: string): boolean {
  const root = rootDomain();
  if (hostname === root || hostname.endsWith(`.${root}`)) return true;
  if (LEGACY_ACCOUNT_HOSTS.includes(hostname)) return true;
  try {
    if (hostname === new URL(siteUrl("portal")).hostname) return true;
  } catch {
    // 覆盖变量不是合法 URL 时，下面按根域判定即可。
  }
  return isLocalHostname(hostname);
}

/** 只有遗留注册域要整页搬到规范主机；bestcodex.app 子域与本地联调不回跳。 */
export function shouldBounceToCanonical(hostname: string): boolean {
  return LEGACY_ACCOUNT_HOSTS.includes(hostname);
}

/** 把遗留官方入口的当前地址改写成规范账号 origin，保留 path / query。 */
export function bounceToCanonicalUrl(currentHref: string): string | null {
  let current: URL;
  try {
    current = new URL(currentHref);
  } catch {
    return null;
  }
  if (!shouldBounceToCanonical(current.hostname)) return null;
  return `${canonicalAccountOrigin()}${current.pathname}${current.search}`;
}

const PRODUCT_PATH: Record<Exclude<SiteId, "portal">, string> = {
  codex: "/codex",
  cc: "/claude",
};

export function siteUrl(site: SiteId): string {
  const overrides: Record<SiteId, string | undefined> = {
    portal: env("VITE_PORTAL_URL"),
    cc: env("VITE_CC_URL"),
    codex: env("VITE_CODEX_URL"),
  };
  const override = overrides[site];
  if (override) return trimSlash(override);

  const root = rootDomain();
  if (site === "portal") return `https://${root}`;
  return `https://${root}${PRODUCT_PATH[site]}`;
}

/** 产品站 origin（默认营销 apex）。覆盖变量若带 /codex 或 /claude，只取主机。 */
export function productSiteOrigin(): string {
  const override = env("VITE_CODEX_URL") ?? env("VITE_CC_URL");
  if (override) {
    try {
      return new URL(trimSlash(override)).origin;
    } catch {
      return trimSlash(override);
    }
  }
  return `https://${rootDomain()}`;
}

export function apiBaseUrl(): string {
  return trimSlash(env("VITE_API_BASE_URL") ?? DEFAULT_API_BASE_URL);
}

/**
 * CCHaven 控制面。它不是身份提供方（账号在 Sub2API），但仍是 CC 桌面端的
 * OAuth token issuer，门户的 `/authorize` 确认页要跨源调它的授权端点。
 */
export function ccControlBaseUrl(): string {
  return trimSlash(env("VITE_CC_CONTROL_URL") ?? DEFAULT_CC_CONTROL_BASE_URL);
}

/**
 * 充值走 Sub2API 托管的收银台，不是本仓接口。
 * 有 access token 时经 /auth/bridge 交接（令牌只放 hash）；没有则直开 /purchase。
 * 桌面端与 CC 控制面仍走无会话的 /purchase。
 */
export function purchaseUrl(accessToken?: string | null): string {
  const checkout = `${apiBaseUrl()}/purchase`;
  const token = accessToken?.trim();
  if (!token) return checkout;
  const hash = new URLSearchParams({ t: token, r: "/purchase" }).toString();
  return `${apiBaseUrl()}/auth/bridge#${hash}`;
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
/** 桌面端约定的帮助入口。若 apex 是门户，产品站镜像页仍可打开。 */
export function helpCanonicalUrl(): string {
  return `https://${rootDomain()}/help`;
}

export function helpProductUrl(path = ""): string {
  const suffix = path ? `/${path.replace(/^\/+/, "")}` : "";
  return `${productSiteOrigin()}/help${suffix}`;
}

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

  const host = url.hostname;
  if (!isOfficialAccountHost(host)) return false;
  if (url.protocol === "https:") return true;
  return url.protocol === "http:" && isLocalHostname(host);
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

export interface SupportChannels {
  qqGroupNumber: string;
  feishuGroupUrl: string;
}

function channelValue(key: string, fallback: string, trimTrailingSlash = false): string {
  const value = env(key);
  if (value === CHANNEL_OFF) return "";
  if (!value) return fallback;
  return trimTrailingSlash ? trimSlash(value) : value;
}

/** 右下角客服气泡的社群入口。空字符串表示这条通道未提供，界面不渲染。 */
export function supportChannels(): SupportChannels {
  return {
    qqGroupNumber: channelValue("VITE_SUPPORT_QQ_NUMBER", DEFAULT_SUPPORT_QQ_NUMBER),
    feishuGroupUrl: channelValue("VITE_SUPPORT_FEISHU_URL", DEFAULT_SUPPORT_FEISHU_URL, true),
  };
}
