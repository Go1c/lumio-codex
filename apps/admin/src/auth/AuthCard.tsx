import type { ReactNode } from "react";
import { t } from "../i18n";

/** 登录/两步验证共用的居中卡片，页头带品牌与「内部系统」标识。 */
export function AuthCard({
  title,
  subtitle,
  children,
}: {
  title: string;
  subtitle?: string;
  children: ReactNode;
}) {
  return (
    <div className="auth-page">
      <div className="auth-card">
        <div className="auth-brand">
          <span className="brand-mark" aria-hidden="true" />
          <span className="brand-name">{t("brand.name")}</span>
          <span className="brand-suffix">{t("brand.suffix")}</span>
        </div>
        <h1>{title}</h1>
        {subtitle && <p className="sub">{subtitle}</p>}
        {children}
      </div>
    </div>
  );
}
