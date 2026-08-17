import type { ReactNode } from "react";
import { Link } from "react-router-dom";

import { siteUrl, type AccountLinks, type SiteId } from "../config";
import { ClaudeMark, LumioLogo, OpenAIMark } from "./brand";
import { SupportBubble } from "./SupportBubble";

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

/** 顶栏 mark 跟站点走：门户用 Lumio 字标，产品站用各自的品牌图标。 */
function BrandMark({ site }: { site?: SiteId }) {
  if (site === "codex") return <OpenAIMark size={22} className="mark-codex" />;
  if (site === "cc") return <ClaudeMark size={22} className="mark-claude" />;
  return <LumioLogo size={22} />;
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
    <div className={`site theme-${site ?? "portal"}`}>
      <a className="skip-link" href="#main">
        跳到主要内容
      </a>
      <header className="site-header">
        <SiteLink href={brand.href ?? "/"} className="logo">
          <span className="mark" aria-hidden="true">
            <BrandMark site={site} />
          </span>
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
      <SupportBubble />
    </div>
  );
}
