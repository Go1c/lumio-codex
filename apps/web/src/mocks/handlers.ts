import { HttpResponse, http, type HttpHandler } from "msw";

import {
  DEMO_CODE,
  DEMO_PASSWORD,
  SCENARIO_EMAILS,
  VALID_INVITE_CODE,
  VALID_RESET_TOKEN,
  db,
  setLoggedIn,
} from "./db";

/**
 * MSW 处理器：严格对齐 `services/cchaven-control/internal/api` 的真实响应。
 *
 * - 成功：`{"data": ...}`（internal/httpx.JSON）
 * - 失败：`{"error":{"code","message","details"}}`（internal/httpx.Fail），
 *   message 逐字取自 internal/i18n 字典。
 */

const API = "*/api/v1";

export function ok<T>(data: T, status = 200) {
  return HttpResponse.json({ data }, { status });
}

export function fail(
  status: number,
  code: string,
  message: string,
  details?: Record<string, unknown>,
) {
  return HttpResponse.json({ error: { code, message, details } }, { status });
}

/** 未登录时 requireUser 返回的错误，前端据此走「无权限」态。 */
const unauthorized = () => fail(401, "unauthorized", "请先登录。");

export const handlers: HttpHandler[] = [
  // —— 公共配置 ——
  http.get(`${API}/config/public`, () => ok(db.config)),

  http.get(`${API}/billing/plan`, () =>
    ok({
      name: "CC避风港包月",
      amount_cents: db.config.pricing.amount_cents,
      currency: db.config.pricing.currency,
      period_unit: "month",
      channels: ["alipay", "wechat", "card"],
    }),
  ),

  // —— 注册 / 验证 / 登录 / 找回 ——
  http.post(`${API}/auth/register`, async ({ request }) => {
    const { email } = (await request.json()) as { email: string; password: string };

    if (email === SCENARIO_EMAILS.taken) return fail(409, "email_taken", "该邮箱已注册。");
    if (email === SCENARIO_EMAILS.rateLimited) {
      return fail(429, "rate_limited", "尝试次数过多，请 1 分钟后再试。", {
        retry_after_seconds: 60,
      });
    }

    db.codeAttemptsRemaining = 5;
    return ok({ email, next: "verify_email", dev_code: DEMO_CODE }, 201);
  }),

  http.post(`${API}/auth/verify-email`, async ({ request }) => {
    const { email, code } = (await request.json()) as { email: string; code: string };

    if (db.codeAttemptsRemaining <= 0) {
      return fail(410, "code_expired", "该验证码已过期，请重新发送。");
    }
    if (code !== DEMO_CODE) {
      db.codeAttemptsRemaining -= 1;
      if (db.codeAttemptsRemaining <= 0) {
        return fail(410, "code_expired", "该验证码已过期，请重新发送。");
      }
      return fail(
        400,
        "code_invalid",
        `验证码不正确，还剩 ${db.codeAttemptsRemaining} 次尝试机会。`,
        { attempts_remaining: db.codeAttemptsRemaining },
      );
    }

    setLoggedIn(true);
    db.user = { ...db.user, email };
    return ok({ user: db.user, entitlement: db.entitlement });
  }),

  http.post(`${API}/auth/verification-code/resend`, () => {
    db.codeAttemptsRemaining = 5;
    return ok({ retry_after_seconds: 60, dev_code: DEMO_CODE }, 202);
  }),

  http.post(`${API}/auth/login`, async ({ request }) => {
    const { email, password } = (await request.json()) as { email: string; password: string };

    switch (email) {
      case SCENARIO_EMAILS.unverified:
        return fail(403, "email_unverified", "你的邮箱尚未验证。");
      case SCENARIO_EMAILS.locked:
        return fail(423, "account_locked", "尝试次数过多，请 15 分钟后再试。", {
          retry_after_seconds: 900,
        });
      case SCENARIO_EMAILS.disabled:
        return fail(403, "account_disabled", "账号已停用，请联系支持。");
      default:
        break;
    }

    if (password !== DEMO_PASSWORD) {
      return fail(401, "invalid_credentials", "邮箱或密码不正确。");
    }

    setLoggedIn(true);
    db.user = { ...db.user, email };
    return ok({ user: db.user, entitlement: db.entitlement });
  }),

  http.post(`${API}/auth/password/forgot`, async ({ request }) => {
    const { email } = (await request.json()) as { email: string };
    return ok(
      {
        message: `如 ${email} 已注册账号，你将很快收到重设链接。`,
        dev_token: VALID_RESET_TOKEN,
      },
      202,
    );
  }),

  http.get(`${API}/auth/password/reset/:token`, ({ params }) => {
    if (params.token !== VALID_RESET_TOKEN) {
      return fail(410, "reset_link_invalid", "该链接已过期或已被使用。");
    }
    return ok({ valid: true, email_masked: "m***y@example.com" });
  }),

  http.post(`${API}/auth/password/reset`, async ({ request }) => {
    const { token } = (await request.json()) as { token: string };
    if (token !== VALID_RESET_TOKEN) {
      return fail(410, "reset_link_invalid", "该链接已过期或已被使用。");
    }
    setLoggedIn(false);
    return ok({ message: "密码已更新，所有设备已退出登录。" });
  }),

  http.post(`${API}/auth/refresh`, () => {
    if (!db.loggedIn) return fail(401, "session_expired", "登录已过期，请重新登录。");
    return ok({ expires_in: 900 });
  }),

  http.post(`${API}/auth/logout`, () => {
    setLoggedIn(false);
    return new HttpResponse(null, { status: 204 });
  }),

  http.get(`${API}/auth/session`, () =>
    db.loggedIn ? ok({ user: db.user, entitlement: db.entitlement }) : unauthorized(),
  ),

  // —— 邀请（公开） ——
  // current 是静态段，必须排在 :code 之前，否则会被当成邀请码匹配掉（后端 chi 路由树同理）。
  http.get(`${API}/invites/current`, () =>
    ok(
      db.inviteAttributed
        ? { attributed: true, inviter: "Alex", trial_days: db.config.invite.trial_days }
        : { attributed: false },
    ),
  ),

  http.get(`${API}/invites/:code`, ({ params }) => {
    const code = String(params.code);
    if (code !== VALID_INVITE_CODE) {
      return ok({ valid: false, code, trial_days: db.config.invite.trial_days });
    }
    // 真实后端在这里下发 30 天的 HttpOnly `cch_ref`；mock 用一个状态位代替。
    db.inviteAttributed = true;
    return ok({
      valid: true,
      code,
      inviter: "Alex",
      trial_days: db.config.invite.trial_days,
    });
  }),

  // —— 账户 ——
  http.get(`${API}/me/`, () =>
    db.loggedIn ? ok({ user: db.user, entitlement: db.entitlement }) : unauthorized(),
  ),

  http.patch(`${API}/me/`, async ({ request }) => {
    if (!db.loggedIn) return unauthorized();
    const { display_name } = (await request.json()) as { display_name: string };
    db.user = { ...db.user, display_name };
    return ok(db.user);
  }),

  http.get(`${API}/me/entitlement`, () => (db.loggedIn ? ok(db.entitlement) : unauthorized())),

  http.post(`${API}/me/password`, async ({ request }) => {
    if (!db.loggedIn) return unauthorized();
    const { current_password } = (await request.json()) as { current_password: string };
    if (current_password !== DEMO_PASSWORD) {
      return fail(400, "current_password_invalid", "当前密码不正确。");
    }
    db.sessions = db.sessions.filter((session) => session.current);
    return ok({ message: "密码已更新，其他设备已退出登录。" });
  }),

  http.post(`${API}/me/email-change`, async ({ request }) => {
    if (!db.loggedIn) return unauthorized();
    const { new_email } = (await request.json()) as { new_email: string };
    db.pendingEmailChange = new_email;
    return ok({ sent: true, dev_code: DEMO_CODE }, 202);
  }),

  http.post(`${API}/me/email-change/verify`, async ({ request }) => {
    if (!db.loggedIn) return unauthorized();
    const { code } = (await request.json()) as { code: string };
    if (code !== DEMO_CODE) {
      return fail(400, "code_invalid", "验证码不正确，还剩 4 次尝试机会。", {
        attempts_remaining: 4,
      });
    }
    if (db.pendingEmailChange) db.user = { ...db.user, email: db.pendingEmailChange };
    db.pendingEmailChange = null;
    return ok(db.user);
  }),

  http.delete(`${API}/me/email-change`, () => {
    db.pendingEmailChange = null;
    return new HttpResponse(null, { status: 204 });
  }),

  http.get(`${API}/me/sessions`, () => (db.loggedIn ? ok({ items: db.sessions }) : unauthorized())),

  http.delete(`${API}/me/sessions/:id`, ({ params }) => {
    if (!db.loggedIn) return unauthorized();
    db.sessions = db.sessions.filter((session) => session.id !== params.id);
    return new HttpResponse(null, { status: 204 });
  }),

  http.post(`${API}/me/sessions/revoke-others`, () => {
    if (!db.loggedIn) return unauthorized();
    const revoked = db.sessions.filter((session) => !session.current).length;
    db.sessions = db.sessions.filter((session) => session.current);
    return ok({ revoked });
  }),

  http.get(`${API}/me/referrals`, () => (db.loggedIn ? ok(db.referrals) : unauthorized())),

  http.post(`${API}/me/deletion`, () => {
    if (!db.loggedIn) return unauthorized();
    const effective = new Date(Date.now() + 7 * 86400_000).toISOString();
    db.user = { ...db.user, deletion_requested_at: new Date().toISOString(), deletion_effective_at: effective };
    return ok({ effective_at: effective });
  }),

  http.delete(`${API}/me/deletion`, () => {
    if (!db.loggedIn) return unauthorized();
    db.user = { ...db.user, deletion_requested_at: undefined, deletion_effective_at: undefined };
    return new HttpResponse(null, { status: 204 });
  }),

  // —— 付款（跳支付服务商托管页） ——
  http.post(`${API}/billing/checkout`, async ({ request }) => {
    if (!db.loggedIn) return unauthorized();
    const { channel } = (await request.json()) as { channel: string };
    return ok({
      order_no: "CC20260812-000042",
      pay_url: `https://pay.example.com/checkout/${channel}/CC20260812-000042`,
      amount_cents: db.config.pricing.amount_cents,
      currency: db.config.pricing.currency,
      expires_at: new Date(Date.now() + 30 * 60_000).toISOString(),
    });
  }),

  http.get(`${API}/billing/orders`, () => (db.loggedIn ? ok({ items: [] }) : unauthorized())),

  // —— OAuth 授权页 ——
  http.get(`${API}/oauth/authorize/context`, ({ request }) => {
    const query = new URL(request.url).searchParams;

    if (query.get("client_id") !== "cchaven-desktop") {
      return fail(400, "invalid_request", "授权请求参数不正确。", { reason: "未知的 client_id" });
    }
    if (query.get("code_challenge_method") !== "S256") {
      return fail(400, "invalid_request", "授权请求参数不正确。", {
        reason: "code_challenge_method 必须为 S256",
      });
    }

    const scopeLabels: Record<string, string> = {
      profile: "读取你的账号邮箱与订阅状态",
      workspace: "代表你连接与同步你的工作区",
      offline_access: "在你未打开浏览器时保持登录",
    };
    const scopes = (query.get("scope") ?? "")
      .split(/\s+/)
      .filter(Boolean)
      .map((id) => ({ id, label: scopeLabels[id] ?? id }));

    return ok({
      client_name: "CC避风港 APP",
      scopes,
      redirect_kind: (query.get("redirect_uri") ?? "").startsWith("http://") ? "loopback" : "scheme",
      logged_in: db.loggedIn,
      ...(db.loggedIn ? { email: db.user.email } : {}),
    });
  }),

  http.post(`${API}/oauth/authorize`, ({ request }) => {
    if (!db.loggedIn) return unauthorized();

    const query = new URL(request.url).searchParams;
    const redirectURI = query.get("redirect_uri") ?? "";
    const state = query.get("state");
    const code = "mock-authorization-code-8f2a1c";
    const separator = redirectURI.includes("?") ? "&" : "?";
    const params = new URLSearchParams({ code, ...(state ? { state } : {}) });

    return ok({
      code,
      redirect_to: `${redirectURI}${separator}${params.toString()}`,
      expires_in: 300,
    });
  }),
];
