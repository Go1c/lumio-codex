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

/** 外壳里的固定文案。默认中文；英文页传 `EN_SITE_LABELS` 覆盖。 */
export interface SiteShellLabels {
  skipToContent: string;
  productNav: string;
  accountNav: string;
  siteLinks: string;
  help: string;
  account: string;
  login: string;
  signup: string;
  download: string;
  productFooter: string;
}

export interface SiteShellProps {
  brand: { name: string; nameEn?: string; href?: string };
  site?: SiteId;
  /** 产品站顶栏中间是 Codex / Claude 切换，所以这些条目渲染在右侧账号区之前。 */
  nav?: SiteNavItem[];
  account?: SiteAccountState;
  accountLinks: AccountLinks;
  /**
   * 页脚互链。只进 sitemap 的页面对爬虫是孤岛——百度尤其依赖站内链接图去发现和
   * 评估页面，所以指南、帮助与语言版本必须每页可达。
   */
  footerLinks?: SiteNavItem[];
  footerExtra?: ReactNode;
  /** 产品站顶栏「下载」的落点。帮助页应指回首页锚点。 */
  downloadHref?: string;
  helpHref?: string;
  /** 只需覆盖要改的几条，其余用中文默认值。 */
  labels?: Partial<SiteShellLabels>;
  children: ReactNode;
}

const PRODUCT_SWITCH = [
  { id: "codex" as const, label: "Codex", to: "/codex" },
  { id: "cc" as const, label: "Claude", to: "/claude" },
];

const ZH_LABELS: SiteShellLabels = {
  skipToContent: "跳到主要内容",
  productNav: "产品",
  accountNav: "账号导航",
  siteLinks: "站内导航",
  help: "帮助",
  account: "账户",
  login: "登录",
  signup: "注册",
  download: "下载",
  productFooter:
    "BestCodex 是独立项目，与 OpenAI、Anthropic 无从属关系。OpenAI、ChatGPT、Codex、Claude、Anthropic 为其各自所有者的商标。官方应用需单独安装。开源软件，AGPL-3.0-only。",
};

/** 英文页用。免责声明的口径必须与中文版一致，只是换语言。 */
export const EN_SITE_LABELS: SiteShellLabels = {
  skipToContent: "Skip to main content",
  productNav: "Products",
  accountNav: "Account",
  siteLinks: "Site links",
  help: "Help",
  account: "Account",
  login: "Sign in",
  signup: "Sign up",
  download: "Download",
  productFooter:
    "BestCodex is an independent project, not affiliated with OpenAI or Anthropic. OpenAI, ChatGPT, Codex, Claude, and Anthropic are trademarks of their respective owners. Official applications must be installed separately. Open source, AGPL-3.0-only.",
};

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

function BrandMark({ site, name }: { site?: SiteId; name: string }) {
  if (isProductSite(site) || name === "BestCodex") {
    return <BestCodexMark size={22} className="mark-bestcodex" />;
  }
  return <LumioLogo size={22} />;
}

export function SiteShell({
  brand,
  site,
  nav = [],
  account,
  accountLinks,
  footerLinks = [],
  footerExtra,
  downloadHref = "/#downloads",
  helpHref = "/help",
  labels,
  children,
}: SiteShellProps) {
  const product = isProductSite(site);
  const t = labels ? { ...ZH_LABELS, ...labels } : ZH_LABELS;

  return (
    <div className={`site theme-${site ?? "portal"}${product ? " is-product" : ""}`}>
      <a className="skip-link" href="#main">
        {t.skipToContent}
      </a>
      <header className={`site-header${product ? " site-header-product" : ""}`}>
        <SiteLink href={brand.href ?? "/"} className="logo">
          <span className="mark" aria-hidden="true">
            <BrandMark site={site} name={brand.name} />
          </span>
          {brand.name}
          {brand.nameEn && <span className="logo-en">{brand.nameEn}</span>}
        </SiteLink>
        {product ? (
          <nav className="product-switch" aria-label={t.productNav}>
            {PRODUCT_SWITCH.map((item) => (
              <Link
                key={item.id}
                to={item.to}
                aria-current={site === item.id ? "page" : undefined}
              >
                {item.label}
              </Link>
            ))}
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
        <nav className="site-nav site-nav-end" aria-label={t.accountNav}>
          {product &&
            nav.map((item) => (
              <SiteLink key={item.href} href={item.href}>
                {item.label}
              </SiteLink>
            ))}
          {product && <SiteLink href={helpHref}>{t.help}</SiteLink>}
          {account?.status === "authenticated" && (
            <SiteLink href={accountLinks.account} className="account-link">
              <span className="avatar" aria-hidden="true">
                {account.email?.[0]?.toUpperCase() ?? "U"}
              </span>
              {t.account}
            </SiteLink>
          )}
          {account?.status === "anonymous" && (
            <>
              <SiteLink href={accountLinks.login}>{t.login}</SiteLink>
              <SiteLink href={accountLinks.signup} className="btn btn-primary btn-sm">
                {t.signup}
              </SiteLink>
            </>
          )}
          {product && (
            <SiteLink href={downloadHref} className="btn-download" style={DOWNLOAD_BTN_STYLE}>
              {t.download}
            </SiteLink>
          )}
        </nav>
      </header>

      <main id="main">{children}</main>

      {product ? (
        <footer className="site-footer site-footer-product">
          {footerLinks.length > 0 && (
            <nav className="footer-nav-product" aria-label={t.siteLinks}>
              {footerLinks.map((item) => (
                <SiteLink key={item.href} href={item.href}>
                  {item.label}
                </SiteLink>
              ))}
            </nav>
          )}
          <p>{t.productFooter}</p>
          {footerExtra}
        </footer>
      ) : (
        <footer className="site-footer">
          <div className="footer-main">
            <div className="footer-brand">
              <span className="logo">
                <span className="mark" aria-hidden="true">
                  <BrandMark site={site} name={brand.name} />
                </span>
                {brand.name}
              </span>
              <p className="footer-tagline">
                一个账号，一个 BestCodex 启动器。注册、登录与余额都在这里完成。
              </p>
            </div>
            <nav className="footer-links" aria-label="站点互链">
              <a href={siteUrl("codex")}>BestCodex</a>
            </nav>
          </div>
          <div className="footer-meta">
            <span>© 2026 {brand.name}</span>
            {footerExtra}
          </div>
        </footer>
      )}
    </div>
  );
}
