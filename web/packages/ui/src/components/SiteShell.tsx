import type { CSSProperties, ReactNode } from "react";
import { Link } from "react-router-dom";

import { rootDomain, siteUrl, type AccountLinks, type SiteId } from "../config";
import { BestCodexMark, LumioLogo } from "./brand";

export interface SiteNavItem {
  label: string;
  href: string;
}

export interface SiteAccountState {
  /** loading 时不渲染任何账号入口，避免先闪「登录」再跳成「账户」。 */
  status: "loading" | "anonymous" | "authenticated";
  email?: string;
}

export interface SiteShellProps {
  brand: { name: string; nameEn?: string; href?: string };
  site?: SiteId;
  nav?: SiteNavItem[];
  account?: SiteAccountState;
  accountLinks: AccountLinks;
  footerExtra?: ReactNode;
  /** 产品站顶栏「下载」的落点。帮助页应指回首页锚点。 */
  downloadHref?: string;
  helpHref?: string;
  children: ReactNode;
}

const SITE_LABELS: Record<SiteId, string> = {
  portal: "Lumio 官网",
  cc: "CC避风港",
  codex: "Lumio Codex",
};

const PRODUCT_FOOTER =
  "BestCodex 是独立项目，与 OpenAI、Anthropic 无从属关系。OpenAI、ChatGPT、Codex、Claude、Anthropic 为其各自所有者的商标。官方应用需单独安装。开源软件，AGPL-3.0-only。";

const DOWNLOAD_BTN_STYLE: CSSProperties = {
  color: "#ffffff",
  backgroundColor: "#1d1d1f",
};

function isProductSite(site?: SiteId): site is "cc" | "codex" {
  return site === "cc" || site === "codex";
}

function isFamilyHref(href: string): boolean {
  if (href.startsWith("/")) return true;
  try {
    const host = new URL(href).hostname;
    const root = rootDomain();
    return host === root || host.endsWith(`.${root}`);
  } catch {
    return false;
  }
}

/** 站内路径用 Link（不整页刷新），跨站绝对地址用普通 a。同家族站点不新开标签。 */
export function SiteLink({
  href,
  className,
  children,
  style,
}: {
  href: string;
  className?: string;
  children: ReactNode;
  style?: CSSProperties;
}) {
  if (href.startsWith("/")) {
    return (
      <Link to={href} className={className} style={style}>
        {children}
      </Link>
    );
  }
  const external = /^https?:/.test(href);
  const newTab = external && !isFamilyHref(href);
  return (
    <a href={href} className={className} style={style} {...(newTab ? { target: "_blank", rel: "noreferrer" } : {})}>
      {children}
    </a>
  );
}

function BrandMark({ site }: { site?: SiteId }) {
  if (isProductSite(site)) return <BestCodexMark size={22} className="mark-bestcodex" />;
  return <LumioLogo size={22} />;
}

export function SiteShell({
  brand,
  site,
  nav = [],
  account,
  accountLinks,
  footerExtra,
  downloadHref = "/#downloads",
  helpHref = "/help",
  children,
}: SiteShellProps) {
  const product = isProductSite(site);
  const siblings = (Object.keys(SITE_LABELS) as SiteId[]).filter((id) => id !== site);

  return (
    <div className={`site theme-${site ?? "portal"}${product ? " is-product" : ""}`}>
      <a className="skip-link" href="#main">
        跳到主要内容
      </a>
      <header className={`site-header${product ? " site-header-product" : ""}`}>
        <SiteLink href={brand.href ?? "/"} className="logo">
          <span className="mark" aria-hidden="true">
            <BrandMark site={site} />
          </span>
          {brand.name}
          {brand.nameEn && <span className="logo-en">{brand.nameEn}</span>}
        </SiteLink>
        {product ? (
          <nav className="product-switch" aria-label="产品">
            <a href={siteUrl("codex")} aria-current={site === "codex" ? "page" : undefined}>
              Codex
            </a>
            <a href={siteUrl("cc")} aria-current={site === "cc" ? "page" : undefined}>
              Claude
            </a>
          </nav>
        ) : (
          nav.length > 0 && (
            <nav className="site-nav" aria-label="主导航">
              {nav.map((item) => (
                <SiteLink key={item.href} href={item.href}>
                  {item.label}
                </SiteLink>
              ))}
            </nav>
          )
        )}
        <span className="spacer" />
        <nav className="site-nav site-nav-end" aria-label="账号导航">
          {product && <SiteLink href={helpHref}>帮助</SiteLink>}
          {account?.status === "authenticated" && (
            <SiteLink href={accountLinks.account} className="account-link">
              <span className="avatar" aria-hidden="true">
                {account.email?.[0]?.toUpperCase() ?? "U"}
              </span>
              账户
            </SiteLink>
          )}
          {account?.status === "anonymous" && (
            <>
              <SiteLink href={accountLinks.login}>登录</SiteLink>
              <SiteLink href={accountLinks.signup} className="btn btn-primary btn-sm">
                注册
              </SiteLink>
            </>
          )}
          {product && (
            <SiteLink href={downloadHref} className="btn-download" style={DOWNLOAD_BTN_STYLE}>
              下载
            </SiteLink>
          )}
        </nav>
      </header>

      <main id="main">{children}</main>

      {product ? (
        <footer className="site-footer site-footer-product">
          <p>{PRODUCT_FOOTER}</p>
          {footerExtra}
        </footer>
      ) : (
        <footer className="site-footer">
          <div className="footer-main">
            <div className="footer-brand">
              <span className="logo">
                <span className="mark" aria-hidden="true">
                  <LumioLogo size={22} />
                </span>
                Lumio
              </span>
              <p className="footer-tagline">
                一个账号，两件趁手的 AI 开发利器。注册、登录与余额统一在 Lumio 官网完成。
              </p>
            </div>
            <nav className="footer-links" aria-label="站点互链">
              {siblings.map((id) => (
                <a key={id} href={siteUrl(id)}>
                  {SITE_LABELS[id]}
                </a>
              ))}
            </nav>
          </div>
          <div className="footer-meta">
            <span>© 2026 Lumio</span>
            {footerExtra}
          </div>
        </footer>
      )}
    </div>
  );
}
