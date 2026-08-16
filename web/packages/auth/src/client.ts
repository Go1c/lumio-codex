/**
 * Sub2API HTTP 客户端。
 *
 * 约定（对齐 codex/crates/codex-plus-core/src/lumio/api.rs）：
 * - 统一信封 `{ code, message, reason, data }`，`code === 0` 才算成功；
 * - 限流中间件不套信封，所以状态码分支必须能在没有信封的情况下工作；
 * - 2FA 挑战是 HTTP 200 的成功响应，只能靠 `requires_2fa` 判断。
 */

import { apiBaseUrl } from "@lumio/ui/config";

import { networkErrorCode, normalizeReason, errorText, type LumioErrorCode } from "./errors";

export interface AgreementDocument {
  id: string;
  title: string;
  contentMd: string;
}

export interface PublicSettings {
  registrationEnabled: boolean;
  emailVerifyEnabled: boolean;
  emailSuffixWhitelist: string[];
  passwordResetEnabled: boolean;
  agreementEnabled: boolean;
  agreementRevision: string;
  agreementDocuments: AgreementDocument[];
}

export interface TokenPair {
  accessToken: string;
  refreshToken: string;
  expiresIn: number;
}

export interface AccountProfile {
  id: number;
  email: string;
  balance: number;
  status: string;
}

export type AuthOutcome =
  | { kind: "tokens"; tokens: TokenPair; profile: AccountProfile }
  | { kind: "2fa"; tempToken: string; maskedEmail: string };

export interface RegisterInput {
  email: string;
  password: string;
  verifyCode?: string;
  invitationCode?: string;
  /** 邀请返利归因码（`?aff=`）；非法/失效由服务端静默忽略，绝不阻断注册。 */
  affCode?: string;
}

export class LumioApiError extends Error {
  readonly code: LumioErrorCode;

  constructor(code: LumioErrorCode) {
    super(errorText(code));
    this.name = "LumioApiError";
    this.code = code;
  }
}

type Json = Record<string, unknown>;

function url(path: string): string {
  return `${apiBaseUrl()}${path}`;
}

function str(value: unknown, fallback = ""): string {
  return typeof value === "string" ? value : fallback;
}

function num(value: unknown, fallback = 0): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function bool(value: unknown): boolean {
  return value === true;
}

function list(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

async function send(path: string, init: RequestInit): Promise<Response> {
  try {
    return await fetch(url(path), init);
  } catch {
    // fetch 的异常文本会带上完整 URL 与查询串，一律折叠成稳定码。
    throw new LumioApiError(networkErrorCode());
  }
}

async function readEnvelope(response: Response): Promise<Json> {
  const text = await response.text().catch(() => "");
  let body: Json | null = null;
  try {
    body = JSON.parse(text) as Json;
  } catch {
    body = null;
  }

  if (!response.ok) {
    throw new LumioApiError(normalizeReason(response.status, str(body?.reason) || null));
  }
  if (!body) {
    throw new LumioApiError(networkErrorCode());
  }
  if (num(body.code, -1) !== 0) {
    throw new LumioApiError(normalizeReason(response.status, str(body.reason) || null));
  }
  const data = body.data;
  if (data === undefined || data === null || typeof data !== "object") {
    throw new LumioApiError(networkErrorCode());
  }
  return data as Json;
}

function jsonInit(body: unknown, accessToken?: string): RequestInit {
  const headers: Record<string, string> = {
    Accept: "application/json",
    "Content-Type": "application/json",
  };
  if (accessToken) headers.Authorization = `Bearer ${accessToken}`;
  return { method: "POST", headers, body: JSON.stringify(body) };
}

export async function fetchPublicSettings(): Promise<PublicSettings> {
  const response = await send("/api/v1/settings/public", {
    method: "GET",
    headers: { Accept: "application/json" },
  });
  const data = await readEnvelope(response);

  return {
    registrationEnabled: bool(data.registration_enabled),
    emailVerifyEnabled: bool(data.email_verify_enabled),
    emailSuffixWhitelist: list(data.registration_email_suffix_whitelist)
      .map((item) => str(item))
      .filter(Boolean),
    passwordResetEnabled: bool(data.password_reset_enabled),
    agreementEnabled: bool(data.login_agreement_enabled),
    agreementRevision: str(data.login_agreement_revision),
    agreementDocuments: list(data.login_agreement_documents).map((item) => {
      const doc = (item ?? {}) as Json;
      return { id: str(doc.id), title: str(doc.title), contentMd: str(doc.content_md) };
    }),
  };
}

export async function sendVerifyCode(email: string): Promise<number> {
  const response = await send("/api/v1/auth/send-verify-code", jsonInit({ email }));
  const data = await readEnvelope(response);
  return num(data.countdown, 60);
}

export async function register(input: RegisterInput): Promise<AuthOutcome> {
  const body: Json = { email: input.email, password: input.password };
  if (input.verifyCode) body.verify_code = input.verifyCode;
  if (input.invitationCode) body.invitation_code = input.invitationCode;
  if (input.affCode) body.aff_code = input.affCode;

  const response = await send("/api/v1/auth/register", jsonInit(body));
  return readAuthOutcome(response);
}

export async function login(email: string, password: string): Promise<AuthOutcome> {
  const response = await send("/api/v1/auth/login", jsonInit({ email, password }));
  return readAuthOutcome(response);
}

export async function loginTwoFactor(tempToken: string, totpCode: string): Promise<AuthOutcome> {
  const response = await send(
    "/api/v1/auth/login/2fa",
    jsonInit({ temp_token: tempToken, totp_code: totpCode }),
  );
  return readAuthOutcome(response);
}

export async function refreshTokens(refreshToken: string): Promise<TokenPair> {
  const response = await send("/api/v1/auth/refresh", jsonInit({ refresh_token: refreshToken }));
  const data = await readEnvelope(response);
  return tokenPair(data);
}

export async function logout(refreshToken: string): Promise<void> {
  const response = await send("/api/v1/auth/logout", jsonInit({ refresh_token: refreshToken }));
  await readEnvelope(response).catch(() => ({}));
}

export async function fetchProfile(accessToken: string): Promise<AccountProfile> {
  const response = await send("/api/v1/auth/me", {
    method: "GET",
    headers: { Accept: "application/json", Authorization: `Bearer ${accessToken}` },
  });
  const data = await readEnvelope(response);
  return profileOf(data);
}

function tokenPair(data: Json): TokenPair {
  return {
    accessToken: str(data.access_token),
    refreshToken: str(data.refresh_token),
    expiresIn: num(data.expires_in),
  };
}

function profileOf(data: Json): AccountProfile {
  return {
    id: num(data.id),
    email: str(data.email),
    balance: num(data.balance),
    status: str(data.status),
  };
}

/**
 * 邀请返利（affiliate）。字段契约见仓库 docs/ops/05-sub2api-affiliate-contract.md：
 * 数值全部来自接口实时值，门户不得硬编码；`rules` 是后端增强（sub2api PR #307），
 * 旧版本后端没有该键——`null` 表示「未知」，与全 0（不冻结/永久/无上限）语义不同。
 */
export interface AffiliateRuntimeRules {
  rebateFreezeHours: number;
  rebateDurationDays: number;
  rebatePerInviteeCap: number;
  signupBonusEnabled: boolean;
  signupBonusAmount: number;
}

export interface AffiliateTier {
  level: string;
  minInvitees: number;
  minRecharge: number;
  /** 该档返利比例（%）；未配置时为 null。 */
  rebateRatePercent: number | null;
}

export interface AffiliateInvitee {
  userId: number;
  /** 服务端已脱敏（`a***@e***.com`），直接渲染，不再二次处理。 */
  email: string;
  username: string;
  createdAt: string;
  totalRebate: number;
}

export interface AffiliateDetail {
  affCode: string;
  inviterId: number | null;
  affCount: number;
  affQuota: number;
  affFrozenQuota: number;
  affHistoryQuota: number;
  inviteeRechargeTotal: number;
  effectiveRebateRatePercent: number | null;
  tiers: AffiliateTier[];
  currentTier: AffiliateTier | null;
  nextTier: AffiliateTier | null;
  invitees: AffiliateInvitee[];
  rules: AffiliateRuntimeRules | null;
}

export interface AffiliateInviteLog {
  id: number;
  inviteeEmail: string;
  affiliateCode: string;
  success: boolean;
  failureMessage: string;
  bonusAmount: number;
  createdAt: string;
}

export interface AffiliateLogPage {
  items: AffiliateInviteLog[];
  total: number;
  page: number;
  pageSize: number;
}

export interface AffiliateTransferResult {
  transferredQuota: number;
  balance: number;
}

function authorizedInit(accessToken: string, method: "GET" | "POST"): RequestInit {
  const headers: Record<string, string> = {
    Accept: "application/json",
    Authorization: `Bearer ${accessToken}`,
  };
  return method === "POST" ? { method, headers } : { method, headers };
}

function nullableNum(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function tierOf(value: unknown): AffiliateTier {
  const tier = (value ?? {}) as Json;
  return {
    level: str(tier.level),
    minInvitees: num(tier.min_invitees),
    minRecharge: num(tier.min_recharge),
    rebateRatePercent: nullableNum(tier.rebate_rate_percent),
  };
}

function rulesOf(value: unknown): AffiliateRuntimeRules | null {
  if (value === undefined || value === null || typeof value !== "object") return null;
  const rules = value as Json;
  return {
    rebateFreezeHours: num(rules.rebate_freeze_hours),
    rebateDurationDays: num(rules.rebate_duration_days),
    rebatePerInviteeCap: num(rules.rebate_per_invitee_cap),
    signupBonusEnabled: bool(rules.signup_bonus_enabled),
    signupBonusAmount: num(rules.signup_bonus_amount),
  };
}

export async function fetchAffiliate(accessToken: string): Promise<AffiliateDetail> {
  const response = await send("/api/v1/user/aff", authorizedInit(accessToken, "GET"));
  const data = await readEnvelope(response);
  return {
    affCode: str(data.aff_code),
    inviterId: nullableNum(data.inviter_id),
    affCount: num(data.aff_count),
    affQuota: num(data.aff_quota),
    affFrozenQuota: num(data.aff_frozen_quota),
    affHistoryQuota: num(data.aff_history_quota),
    inviteeRechargeTotal: num(data.invitee_recharge_total),
    effectiveRebateRatePercent: nullableNum(data.effective_rebate_rate_percent),
    tiers: list(data.affiliate_tiers).map(tierOf),
    currentTier: data.current_affiliate_tier ? tierOf(data.current_affiliate_tier) : null,
    nextTier: data.next_affiliate_tier ? tierOf(data.next_affiliate_tier) : null,
    invitees: list(data.invitees).map((item) => {
      const invitee = (item ?? {}) as Json;
      return {
        userId: num(invitee.user_id),
        email: str(invitee.email),
        username: str(invitee.username),
        createdAt: str(invitee.created_at),
        totalRebate: num(invitee.total_rebate),
      };
    }),
    rules: rulesOf(data.rules),
  };
}

export async function fetchAffiliateLogs(
  accessToken: string,
  page = 1,
  pageSize = 20,
): Promise<AffiliateLogPage> {
  const response = await send(
    `/api/v1/user/aff/logs?page=${encodeURIComponent(page)}&page_size=${encodeURIComponent(pageSize)}`,
    authorizedInit(accessToken, "GET"),
  );
  const data = await readEnvelope(response);
  return {
    items: list(data.items).map((item) => {
      const log = (item ?? {}) as Json;
      return {
        id: num(log.id),
        inviteeEmail: str(log.invitee_email),
        affiliateCode: str(log.affiliate_code),
        success: bool(log.success),
        failureMessage: str(log.failure_message),
        bonusAmount: num(log.bonus_amount),
        createdAt: str(log.created_at),
      };
    }),
    total: num(data.total),
    page: num(data.page, page),
    pageSize: num(data.page_size, pageSize),
  };
}

export async function transferAffiliateQuota(accessToken: string): Promise<AffiliateTransferResult> {
  const response = await send("/api/v1/user/aff/transfer", authorizedInit(accessToken, "POST"));
  const data = await readEnvelope(response);
  return {
    transferredQuota: num(data.transferred_quota),
    balance: num(data.balance),
  };
}

async function readAuthOutcome(response: Response): Promise<AuthOutcome> {
  const data = await readEnvelope(response);
  if (bool(data.requires_2fa)) {
    return {
      kind: "2fa",
      tempToken: str(data.temp_token),
      maskedEmail: str(data.user_email_masked),
    };
  }
  return {
    kind: "tokens",
    tokens: tokenPair(data),
    profile: profileOf((data.user ?? {}) as Json),
  };
}
