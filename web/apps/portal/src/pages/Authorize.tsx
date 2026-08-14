import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { Link, useLocation, useSearchParams } from "react-router-dom";

import { Banner, ErrorBlock, LoadingBlock, Spinner, Truncated, useToast } from "@lumio/ui";

import {
  approveAuthorization,
  fetchAuthorizeContext,
  messageOfControlError,
  type ApproveResult,
  type AuthorizeContext,
} from "@/lib/ccControl";
import { denyRedirectUrl, goExternal, isAllowedDesktopRedirect } from "@/lib/redirect";
import { usePortalSession } from "@/state/session";

/**
 * CC 桌面端的浏览器授权确认页。
 *
 * 桌面端开系统浏览器到这里（`cchaven/apps/desktop/src-tauri/src/control.rs` 拼的 URL），
 * 用户在门户的账号中心会话下确认，控制面签发授权码后回跳本机回环端口，
 * 桌面端再用 PKCE verifier 换令牌。门户只负责「确认」这一步，不碰令牌交换。
 */
export function Authorize() {
  const [params] = useSearchParams();
  const query = useMemo(() => new URLSearchParams(params), [params]);
  const redirectUri = query.get("redirect_uri") ?? "";

  // 回调地址在接触控制面之前就要判死：拒绝授权时是门户自己拼回跳地址，
  // 拿一个外站地址渲染出「CC避风港 请求授权」的确认页本身就是钓鱼素材。
  if (!isAllowedDesktopRedirect(redirectUri)) {
    return (
      <AuthorizeShell>
        <h2>授权请求无效</h2>
        <p className="sub">回调地址不在允许范围内，无法继续授权。请回到 CC避风港 重新发起登录。</p>
        <Link to="/" className="btn btn-secondary btn-block">
          返回首页
        </Link>
      </AuthorizeShell>
    );
  }

  return <AuthorizeFlow query={query} redirectUri={redirectUri} />;
}

interface ContextState {
  status: "loading" | "ready" | "error";
  data?: AuthorizeContext;
  error?: string;
}

function AuthorizeFlow({ query, redirectUri }: { query: URLSearchParams; redirectUri: string }) {
  const session = usePortalSession();
  const location = useLocation();
  const toast = useToast();

  const search = query.toString();
  const state = query.get("state") ?? "";
  const accessToken = session.accessToken;

  const [context, setContext] = useState<ContextState>({ status: "loading" });
  const [nonce, setNonce] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<ApproveResult | null>(null);
  const [denied, setDenied] = useState(false);

  const reload = useCallback(() => setNonce((value) => value + 1), []);

  useEffect(() => {
    let cancelled = false;
    setContext({ status: "loading" });

    fetchAuthorizeContext(new URLSearchParams(search), accessToken)
      .then((data) => {
        if (!cancelled) setContext({ status: "ready", data });
      })
      .catch((failure: unknown) => {
        if (!cancelled) setContext({ status: "error", error: messageOfControlError(failure) });
      });

    return () => {
      cancelled = true;
    };
  }, [search, accessToken, nonce]);

  async function approve() {
    if (busy || !accessToken) return;
    setBusy(true);
    setError(null);
    try {
      const approved = await approveAuthorization(new URLSearchParams(search), accessToken);
      if (!isAllowedDesktopRedirect(approved.redirectTo)) {
        setError("授权服务返回了不受信任的回调地址，已阻止跳转。请回到 CC避风港 重新发起登录。");
        return;
      }
      setResult(approved);
      // 回环地址与自定义 scheme 都用同一句赋值；唤起失败时页面停在成功态，
      // 用户可以复制授权码手动粘贴回桌面端（桌面端 5.1 超时态的兜底入口）。
      goExternal(approved.redirectTo);
    } catch (failure) {
      setError(messageOfControlError(failure));
    } finally {
      setBusy(false);
    }
  }

  function deny() {
    setDenied(true);
    const target = denyRedirectUrl(redirectUri, state);
    if (target) goExternal(target);
  }

  async function copyCode(code: string) {
    try {
      if (!navigator.clipboard) throw new Error("clipboard unavailable");
      await navigator.clipboard.writeText(code);
      toast("授权码已复制");
    } catch {
      // 剪贴板不可用时授权码仍然显示在页面上：如实提示手动复制，不谎报成功。
      toast("复制未成功，请手动选中复制");
    }
  }

  if (context.status === "loading") {
    return (
      <AuthorizeShell>
        <LoadingBlock label="读取授权请求…" lines={4} />
      </AuthorizeShell>
    );
  }

  if (context.status === "error" || !context.data) {
    return (
      <AuthorizeShell>
        <h2>授权请求无效</h2>
        <ErrorBlock message={context.error ?? ""} onRetry={reload} />
        <Link to="/" className="btn btn-secondary btn-block" style={{ marginTop: 12 }}>
          返回首页
        </Link>
      </AuthorizeShell>
    );
  }

  const data = context.data;

  if (result) {
    return (
      <AuthorizeShell>
        <h2>授权成功</h2>
        <p className="sub">正在唤起 CC避风港。如果没有自动跳转，可以用下面的授权码手动完成登录。</p>
        <div className="code-display">
          <code>{result.code}</code>
          <button
            type="button"
            className="btn btn-secondary"
            onClick={() => void copyCode(result.code)}
          >
            复制
          </button>
        </div>
        <p className="terms">
          授权码 {Math.max(1, Math.round(result.expiresIn / 60))} 分钟内有效，只能使用一次。
        </p>
        <a href={result.redirectTo} className="btn btn-primary btn-block">
          打开 CC避风港
        </a>
      </AuthorizeShell>
    );
  }

  if (denied) {
    return (
      <AuthorizeShell>
        <h2>已拒绝授权</h2>
        <p className="sub">没有向 CC避风港 桌面端授予任何权限，可以关闭此页面。</p>
        <Link to="/" className="btn btn-secondary btn-block">
          返回首页
        </Link>
      </AuthorizeShell>
    );
  }

  const next = `${location.pathname}${location.search}`;
  const loggedIn = session.status === "authenticated" && Boolean(accessToken);
  const email = data.email || session.profile?.email || "";

  return (
    <AuthorizeShell>
      <h2>授权 {data.clientName}</h2>
      <p className="sub">CC避风港 桌面端请求以你的 Lumio 账号身份访问以下内容：</p>

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

      {loggedIn ? (
        <>
          <p className="who">
            当前账号：
            <Truncated text={email} max={32} />
          </p>
          <div className="authorize-actions">
            <button type="button" className="btn btn-secondary" onClick={deny} disabled={busy}>
              拒绝
            </button>
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => void approve()}
              disabled={busy}
            >
              {busy && <Spinner />}
              同意授权
            </button>
          </div>
        </>
      ) : (
        <>
          <Banner kind="warn">请先登录 Lumio 账号，登录后会回到本页继续授权。</Banner>
          <Link to={`/login?next=${encodeURIComponent(next)}`} className="btn btn-primary btn-block">
            去登录
          </Link>
          <p className="auth-links">
            还没有账号？<Link to={`/signup?next=${encodeURIComponent(next)}`}>创建账号</Link>
          </p>
        </>
      )}

      <p className="terms">
        Lumio 不会把你的密码交给任何客户端；授权后可在 CC避风港 桌面端随时退出登录。
      </p>
    </AuthorizeShell>
  );
}

function AuthorizeShell({ children }: { children: ReactNode }) {
  return (
    <div className="auth-page">
      <div className="auth-card wide authorize-card">{children}</div>
    </div>
  );
}
