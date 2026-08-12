import { Link, NavLink, Outlet } from "react-router-dom";

import { useT } from "@/i18n";
import { useSession } from "@/state/session";

const DOCS_URL = "https://docs.cchaven.cn";

export function SiteLayout() {
  const t = useT();
  const { status, user } = useSession();

  return (
    <div className="site">
      <a className="skip-link" href="#main">
        跳到主要内容
      </a>
      <header className="site-header">
        <Link to="/" className="logo">
          <span className="mark" aria-hidden="true" />
          {t("brand.name")} <span className="logo-en">{t("brand.name_en")}</span>
        </Link>
        <nav className="site-nav" aria-label="主导航">
          <NavLink to="/pricing">{t("nav.pricing")}</NavLink>
          <a href={DOCS_URL} target="_blank" rel="noreferrer">
            {t("nav.docs")}
          </a>
          <NavLink to="/download">{t("nav.download")}</NavLink>
        </nav>
        <span className="spacer" />
        <nav className="site-nav site-nav-end" aria-label="账号导航">
          {status === "authenticated" ? (
            <Link to="/account" className="account-link">
              <span className="avatar" aria-hidden="true">
                {user?.email?.[0]?.toUpperCase() ?? "U"}
              </span>
              {t("nav.account")}
            </Link>
          ) : (
            <>
              <Link to="/login">{t("nav.login")}</Link>
              <Link to="/signup" className="btn btn-primary btn-sm">
                {t("nav.start")}
              </Link>
            </>
          )}
        </nav>
      </header>

      <main id="main">
        <Outlet />
      </main>

      <footer className="site-footer">
        <span>{t("footer.copyright")}</span>
        <a href="/terms">{t("footer.terms")}</a>
        <a href="/privacy">{t("footer.privacy")}</a>
        <a href="https://status.cchaven.cn" target="_blank" rel="noreferrer">
          {t("footer.status")}
        </a>
      </footer>
    </div>
  );
}
