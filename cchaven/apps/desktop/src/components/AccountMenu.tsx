import { useEffect, useRef } from "react";
import { t } from "../i18n";
import { formatDate, truncateMiddle } from "../lib/format";
import type { Entitlement, ExternalLinks, SessionView } from "../lib/types";

/** 剩余 ≤3 天时账户入口转橙色并弹出顶部横幅（5.6）。 */
export const EXPIRY_WARNING_DAYS = 3;

export function isExpiringSoon(entitlement?: Entitlement | null): boolean {
  if (!entitlement) return false;
  if (entitlement.expiringSoon) return true;
  return entitlement.status !== "none" && entitlement.daysLeft <= EXPIRY_WARNING_DAYS;
}

/** Subscription line for the account menu header. */
export function entitlementLine(entitlement?: Entitlement | null): string {
  if (!entitlement || entitlement.status === "none") return t("account.none");
  if (entitlement.status === "trialing") {
    return t("account.trialing", { n: entitlement.daysLeft });
  }
  return t("account.subscribed", {
    date: formatDate(entitlement.expiresAt),
    n: entitlement.daysLeft,
  });
}

/** Shorter variant for the sidebar chip. */
export function entitlementChipLine(entitlement?: Entitlement | null): string {
  if (!entitlement || entitlement.status === "none") return t("account.chipNone");
  return entitlement.status === "trialing"
    ? t("account.chipTrialing", { n: entitlement.daysLeft })
    : t("account.chipSubscribed", { n: entitlement.daysLeft });
}

/**
 * 5.6 账户菜单 — the app's only account surface. Every item but 退出登录 opens
 * the website in the system browser; there is no in-app account page.
 */
export function AccountMenu({
  session,
  links,
  onOpenExternal,
  onLogout,
  onClose,
}: {
  session: SessionView;
  links: ExternalLinks;
  onOpenExternal: (url: string) => void;
  onLogout: () => void;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    function onPointerDown(event: MouseEvent) {
      if (!ref.current?.contains(event.target as Node)) onClose();
    }
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") onClose();
    }
    window.addEventListener("mousedown", onPointerDown);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("mousedown", onPointerDown);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [onClose]);

  const entitlement = session.entitlement;
  const warn = isExpiringSoon(entitlement);
  const items: Array<[string, string, string]> = [
    ["🌐", t("account.manage"), links.account],
    ["🎁", t("account.invite"), links.invite],
    ["📖", t("account.docs"), links.docs],
    ["💬", t("account.support"), links.support],
  ];

  return (
    <div className="acct-menu" role="menu" aria-label={t("account.manage")} ref={ref}>
      <div className="acct-menu-head">
        <span className="avatar" aria-hidden="true">
          {session.email.slice(0, 1).toUpperCase() || "U"}
        </span>
        <div style={{ minWidth: 0 }}>
          <div title={session.email}>{truncateMiddle(session.email, 24)}</div>
          <div className={warn ? "plan-line warn" : "plan-line"}>
            {entitlementLine(entitlement)}
          </div>
        </div>
      </div>
      {items.map(([icon, label, url]) => (
        <button
          key={label}
          type="button"
          role="menuitem"
          className="acct-menu-item"
          onClick={() => {
            onClose();
            onOpenExternal(url);
          }}
        >
          <span className="ic" aria-hidden="true">
            {icon}
          </span>
          {label}
        </button>
      ))}
      <div className="acct-menu-sep" />
      <button type="button" role="menuitem" className="acct-menu-item" onClick={onLogout}>
        <span className="ic" aria-hidden="true">
          ↩
        </span>
        {t("account.logout")}
      </button>
    </div>
  );
}
