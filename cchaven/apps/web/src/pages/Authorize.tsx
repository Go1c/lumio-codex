import { useMemo, useState } from "react";
import { Link, useLocation, useSearchParams } from "react-router-dom";

import { approveAuthorization, getAuthorizeContext } from "@/api/endpoints";
import type { ApproveResult, AuthorizeContext } from "@/api/types";
import { useToast } from "@/components/Toast";
import { Banner, ErrorBlock, LoadingBlock, Spinner, Truncated, errorMessage } from "@/components/ui";
import { useResource } from "@/hooks/useResource";
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";

/**
 * APP 授权页 `/authorize`（原型未实现，按交互设计 3.4 与 5.1 自行设计）。
 *
 * 流程：GET /oauth/authorize/context 渲染「谁在请求什么权限」→ 未登录先去登录再回来 →
 * 点「授权」调 POST /oauth/authorize → 跳 redirect_to 唤起 APP，并把 code 展示出来
 * 作为「手动粘贴授权码」兜底（对应 APP 侧 5.1 超时态）。
 *
 * 五态：loading（context 查询中骨架）/ error（参数非法：不可继续 + 回 APP 重来）/
 * empty（不适用）/ disabled（授权提交中按钮禁用）/ 无权限（未登录 → 引导登录）。
 */
export function Authorize() {
  const t = useT();
  const toast = useToast();
  const [params] = useSearchParams();
  const location = useLocation();

  // 授权参数原样透传给服务端校验，前端不做二次解释。
  const query = useMemo(() => new URLSearchParams(params), [params]);

  const context = useResource<AuthorizeContext>(
    (signal) => getAuthorizeContext(query, signal),
    [query.toString()],
  );

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [result, setResult] = useState<ApproveResult | null>(null);
  const [denied, setDenied] = useState(false);

  async function approve() {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const approved = await approveAuthorization(query);
      setResult(approved);
      // 回环地址（http://127.0.0.1:*/callback）与自定义 scheme（cchaven://）都用同一句赋值；
      // 唤起失败时页面停留在成功态，用户可以复制授权码手动粘贴到 APP。
      window.location.assign(approved.redirect_to);
    } catch (err) {
      setError(errorMessage(err, t("common.unknown_error")));
    } finally {
      setBusy(false);
    }
  }

  async function copyCode(code: string) {
    try {
      await navigator.clipboard?.writeText(code);
      toast(t("authorize.copied_code"));
    } catch {
      toast(t("authorize.copied_code"));
    }
  }

  if (context.status === "loading") {
    return (
      <AuthorizeShell>
        <LoadingBlock lines={4} />
      </AuthorizeShell>
    );
  }

  // 参数非法（invalid_request）：无法继续，只能回 APP 重新发起。
  if (context.status === "error" || !context.data) {
    const invalidRequest = context.error instanceof ApiError && context.error.code === "invalid_request";
    return (
      <AuthorizeShell>
        <h2>{t("authorize.invalid_title")}</h2>
        {invalidRequest ? (
          <>
            <p className="sub">{t("authorize.invalid_body")}</p>
            {(context.error as ApiError).reason && (
              <p className="terms">{(context.error as ApiError).reason}</p>
            )}
          </>
        ) : (
          <ErrorBlock
            error={context.error}
            fallback={t("common.unknown_error")}
            onRetry={context.reload}
          />
        )}
        <Link to="/" className="btn btn-secondary btn-block" style={{ marginTop: 12 }}>
          返回首页
        </Link>
      </AuthorizeShell>
    );
  }

  const data = context.data;

  if (result) {
    const expiresMinutes = Math.max(1, Math.round(result.expires_in / 60));
    return (
      <AuthorizeShell>
        <div className="success-check" aria-hidden="true">
          ✓
        </div>
        <h2>{t("authorize.success_title")}</h2>
        <p className="sub">{t("authorize.success_body")}</p>

        <label className="sr-only" htmlFor="auth-code">
          {t("authorize.manual_code")}
        </label>
        <div className="code-display">
          <code id="auth-code">{result.code}</code>
          <button type="button" className="btn btn-secondary" onClick={() => void copyCode(result.code)}>
            {t("authorize.copy_code")}
          </button>
        </div>
        <p className="terms">{t("authorize.manual_hint", { n: expiresMinutes })}</p>

        <a href={result.redirect_to} className="btn btn-primary btn-block" style={{ marginTop: 10 }}>
          {t("authorize.open_app")}
        </a>
      </AuthorizeShell>
    );
  }

  if (denied) {
    return (
      <AuthorizeShell>
        <h2>{t("authorize.deny")}</h2>
        <p className="sub">{t("authorize.denied")}</p>
        <Link to="/" className="btn btn-secondary btn-block">
          返回首页
        </Link>
      </AuthorizeShell>
    );
  }

  const nextTarget = `${location.pathname}${location.search}`;

  return (
    <AuthorizeShell>
      <h2>{t("authorize.title", { client: data.client_name })}</h2>
      <p className="sub">{t("authorize.subtitle", { client: data.client_name })}</p>

      <ul className="scopes">
        {data.scopes.map((scope) => (
          <li key={scope.id}>
            <span aria-hidden="true">✓</span>
            <span>
              {scope.label}
              <code className="scope-id">{scope.id}</code>
            </span>
          </li>
        ))}
      </ul>

      {error && <Banner kind="error">{error}</Banner>}

      {data.logged_in ? (
        <>
          <div className="who">
            {t("authorize.account")}：<Truncated text={data.email ?? ""} max={32} />
          </div>
          <div className="authorize-actions">
            <button type="button" className="btn btn-secondary" onClick={() => setDenied(true)} disabled={busy}>
              {t("authorize.deny")}
            </button>
            <button type="button" className="btn btn-primary" onClick={() => void approve()} disabled={busy}>
              {busy && <Spinner />}
              {busy ? t("authorize.approving") : t("authorize.approve")}
            </button>
          </div>
        </>
      ) : (
        <>
          <Banner kind="warn">{t("authorize.login_required")}</Banner>
          <Link
            to={`/login?next=${encodeURIComponent(nextTarget)}`}
            className="btn btn-primary btn-block"
          >
            {t("authorize.login_cta")}
          </Link>
          <div className="auth-links">
            <Link to={`/signup?next=${encodeURIComponent(nextTarget)}`}>{t("authorize.signup_cta")}</Link>
          </div>
        </>
      )}

      <p className="terms">{t("authorize.security_note")}</p>
    </AuthorizeShell>
  );
}

function AuthorizeShell({ children }: { children: React.ReactNode }) {
  return (
    <div className="auth-page">
      <div className="auth-card wide authorize-card">{children}</div>
    </div>
  );
}
