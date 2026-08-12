import { NavLink } from "react-router-dom";
import { useAuth } from "../auth/AuthProvider";
import { t, type MessageKey } from "../i18n";

const NAV: { to: string; icon: string; label: MessageKey }[] = [
  { to: "/", icon: "📊", label: "nav.dashboard" },
  { to: "/users", icon: "👥", label: "nav.users" },
  { to: "/orders", icon: "🧾", label: "nav.orders" },
  { to: "/settings", icon: "⚙️", label: "nav.settings" },
];

function roleLabel(role: string): string {
  switch (role) {
    case "owner":
      return t("role.owner");
    case "ops":
      return t("role.ops");
    case "support":
      return t("role.support");
    default:
      return role;
  }
}

/** 左侧深色侧栏：品牌 +「运营后台 · 内部系统」标识 + 四个页面入口。 */
export function Sidebar() {
  const { me, logout } = useAuth();

  return (
    <aside className="app-sidebar">
      <div className="sidebar-brand">
        <span className="brand-mark" aria-hidden="true" />
        <span className="brand-name">{t("brand.name")}</span>
        <span className="brand-suffix">{t("brand.suffix")}</span>
      </div>
      <div className="sidebar-internal">{t("brand.internal")}</div>

      <nav aria-label={t("nav.aria")}>
        {NAV.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            end={item.to === "/"}
            className={({ isActive }) => `nav-item ${isActive ? "active" : ""}`}
          >
            <span aria-hidden="true">{item.icon}</span>
            {t(item.label)}
          </NavLink>
        ))}
      </nav>

      <div className="sidebar-bottom">
        {me && (
          <div className="account-chip">
            <span className="avatar" aria-hidden="true">
              {me.email.slice(0, 1).toUpperCase()}
            </span>
            <div className="account-meta">
              <div className="account-email" title={me.email}>
                {me.email}
              </div>
              <div className="account-role">{roleLabel(me.role)}</div>
            </div>
          </div>
        )}
        <button type="button" className="btn btn-sidebar" onClick={() => void logout()}>
          {t("nav.logout")}
        </button>
      </div>
    </aside>
  );
}
