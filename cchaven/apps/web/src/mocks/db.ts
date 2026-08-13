import type {
  Entitlement,
  PublicConfig,
  ReferralOverview,
  SessionView,
  UserView,
} from "@/api/types";

/**
 * Mock 数据源。
 *
 * 本机跑不起 M1 控制面（依赖 PostgreSQL），因此开发与测试都走 MSW；
 * 这里的每个字段都对齐 `services/cchaven-control/internal/service` 里的真实响应结构。
 */

export const DEMO_PASSWORD = "Password123";
export const DEMO_CODE = "123456";
export const VALID_RESET_TOKEN = "valid-token";
export const VALID_INVITE_CODE = "mary8k2f";

/** 用于手工验收各分支的固定账号。 */
export const SCENARIO_EMAILS = {
  ok: "mary@example.com",
  unverified: "unverified@example.com",
  locked: "locked@example.com",
  disabled: "disabled@example.com",
  taken: "taken@example.com",
  rateLimited: "limited@example.com",
};

export interface MockState {
  loggedIn: boolean;
  user: UserView;
  entitlement: Entitlement;
  sessions: SessionView[];
  referrals: ReferralOverview;
  config: PublicConfig;
  /** 验证码剩余尝试次数，对齐后端 max_attempts=5。 */
  codeAttemptsRemaining: number;
  pendingEmailChange: string | null;
  /** 代替真实的 HttpOnly `cch_ref` cookie：打开有效邀请链接后置位，`/invites/current` 据此作答。 */
  inviteAttributed: boolean;
}

function inDays(days: number): string {
  return new Date(Date.now() + days * 86400_000).toISOString();
}

function daysAgo(days: number): string {
  return new Date(Date.now() - days * 86400_000).toISOString();
}

export function initialState(): MockState {
  return {
    loggedIn: false,
    user: {
      id: "U-100986",
      email: SCENARIO_EMAILS.ok,
      display_name: "Mary",
      created_at: daysAgo(120),
    },
    entitlement: {
      status: "active",
      kind: "paid",
      expires_at: inDays(27),
      days_left: 27,
      bonus_days_total: 7,
      expiring_soon: false,
    },
    sessions: [
      {
        id: "11111111-1111-4111-8111-111111111111",
        device_name: "Safari · macOS",
        kind: "web",
        platform_detail: "浏览器 macOS 15",
        last_seen_at: new Date().toISOString(),
        ip_region: "上海",
        current: true,
      },
      {
        id: "22222222-2222-4222-8222-222222222222",
        device_name: "MacBook Pro — CC避风港 APP 1.4.2",
        kind: "app",
        platform_detail: "macOS 15 · Apple Silicon",
        app_version: "1.4.2",
        last_seen_at: new Date(Date.now() - 5 * 60_000).toISOString(),
        ip_region: "上海",
        current: false,
      },
      {
        id: "33333333-3333-4333-8333-333333333333",
        device_name: "Chrome · Windows",
        kind: "web",
        platform_detail: "浏览器 Windows",
        last_seen_at: daysAgo(9),
        ip_region: "杭州",
        current: false,
      },
    ],
    referrals: {
      code: VALID_INVITE_CODE,
      link: `https://cchaven.cn/i/${VALID_INVITE_CODE}`,
      reward_days: 7,
      trial_days: 30,
      invited_count: 1,
      total_bonus_days: 7,
      items: [
        { email_masked: "w***g@gmail.com", status: "activated", bonus_days: 7, at: daysAgo(6) },
        { email_masked: "l***3@qq.com", status: "registered", bonus_days: 0, at: daysAgo(2) },
      ],
    },
    config: {
      pricing: { amount_cents: 6800, currency: "CNY", period_unit: "month" },
      invite: { reward_days: 7, trial_days: 30, reward_enabled: true },
      releases: [
        {
          version: "1.4.2",
          arch: "arm64",
          download_url: "https://dl.cchaven.cn/CCHaven-1.4.2-arm64.dmg",
          min_os: "macOS 13",
          released_at: "2026-08-08T00:00:00Z",
        },
        {
          version: "1.4.2",
          arch: "x86_64",
          download_url: "https://dl.cchaven.cn/CCHaven-1.4.2-x64.dmg",
          min_os: "macOS 13",
          released_at: "2026-08-08T00:00:00Z",
        },
      ],
    },
    codeAttemptsRemaining: 5,
    pendingEmailChange: null,
    inviteAttributed: false,
  };
}

/**
 * 浏览器里把「是否已登录」落到 sessionStorage：真实后端靠 HttpOnly cookie 跨页面刷新保持会话，
 * mock 的内存状态会在整页刷新时丢失，手工验收 `/authorize` 这类需要先登录再跳转的链路会误判。
 * 测试环境不持久化，避免用例之间串味。
 */
const SESSION_KEY = "cchaven.mock_session";
const persistSession = typeof window !== "undefined" && import.meta.env.MODE !== "test";

function readPersistedSession(): boolean {
  if (!persistSession) return false;
  try {
    return window.sessionStorage.getItem(SESSION_KEY) === "1";
  } catch {
    return false;
  }
}

function writePersistedSession(loggedIn: boolean): void {
  if (!persistSession) return;
  try {
    if (loggedIn) window.sessionStorage.setItem(SESSION_KEY, "1");
    else window.sessionStorage.removeItem(SESSION_KEY);
  } catch {
    /* 忽略：只影响手工验收时的会话保持 */
  }
}

export let db: MockState = { ...initialState(), loggedIn: readPersistedSession() };

/** 登录 / 登出统一走这里，顺带同步 mock 会话的持久化。 */
export function setLoggedIn(loggedIn: boolean): void {
  db.loggedIn = loggedIn;
  writePersistedSession(loggedIn);
}

/** 每个测试用例前重置，保证用例之间互不串味。 */
export function resetDb(patch: Partial<MockState> = {}): MockState {
  db = { ...initialState(), ...patch };
  writePersistedSession(db.loggedIn);
  return db;
}
