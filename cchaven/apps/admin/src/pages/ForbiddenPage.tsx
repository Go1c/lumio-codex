import { useAuth } from "../auth/AuthProvider";
import { t } from "../i18n";

/** 五态之一：权限不足（非管理员或权限被回收）时展示 403 页。 */
export function ForbiddenPage() {
  const { backToLogin } = useAuth();

  return (
    <div className="auth-page">
      <div className="auth-card" role="alert">
        <div className="forbidden-art" aria-hidden="true">
          🔒
        </div>
        <h1>{t("forbidden.title")}</h1>
        <p className="sub">{t("forbidden.body")}</p>
        <button type="button" className="btn btn-primary" onClick={backToLogin}>
          {t("forbidden.action")}
        </button>
      </div>
    </div>
  );
}
