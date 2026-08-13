import type { ReactNode } from "react";
import { Link } from "react-router-dom";

import { siteUrl, type AccountLinks, type SiteId } from "../config";

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
  children: ReactNode;
}

const SITE_LABELS: Record<SiteId, string> = {
  portal: "Lumio 官网",
  cc: "CC避风港",
  codex: "Lumio Codex",
};

/** 站内路径用 Link（不整页刷新），跨站绝对地址用普通 a。 */
export function SiteLink({
  href,
  className,
  children,
}: {
  href: string;
  className?: string;
  children: ReactNode;
}) {
  if (href.startsWith("/")) {
    return (
      <Link to={href} className={className}>
        {children}
      </Link>
    );
  }
  const external = /^https?:/.test(href);
  return (
    <a
      href={href}
      className={className}
      {...(external && !href.startsWith(siteUrl("portal"))
        ? { target: "_blank", rel: "noreferrer" }
        : {})}
    >
      {children}
    </a>
  );
}

/** 三站共用的外壳：同一套 Header / Footer，靠 props 决定当前站点、导航与账号区。 */
export function SiteShell({
  brand,
  site,
  nav = [],
  account,
  accountLinks,
  footerExtra,
  children,
}: SiteShellProps) {
  const siblings = (Object.keys(SITE_LABELS) as SiteId[]).filter((id) => id !== site);

  return (
    <div className="site">
      <a className="skip-link" href="#main">
        跳到主要内容
      </a>
      <header className="site-header">
        <SiteLink href={brand.href ?? "/"} className="logo">
          <span className="mark" aria-hidden="true" />
          {brand.name}
          {brand.nameEn && <span className="logo-en">{brand.nameEn}</span>}
        </SiteLink>
        {nav.length > 0 && (
          <nav className="site-nav" aria-label="主导航">
            {nav.map((item) => (
              <SiteLink key={item.href} href={item.href}>
                {item.label}
              </SiteLink>
            ))}
          </nav>
        )}
        <span className="spacer" />
        <nav className="site-nav site-nav-end" aria-label="账号导航">
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
        </nav>
      </header>

      <main id="main">{children}</main>

      <footer className="site-footer">
        <span>© 2026 Lumio</span>
        {siblings.map((id) => (
          <a key={id} href={siteUrl(id)}>
            {SITE_LABELS[id]}
          </a>
        ))}
        {footerExtra}
      </footer>
    </div>
  );
}
