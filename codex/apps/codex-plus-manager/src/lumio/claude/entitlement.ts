import type { LumioAccountSummary } from "../types.ts";

import type { ClaudeEntitlement, ClaudeEntitlementStatus } from "./types.ts";


export const CLAUDE_CONTROL_API = "https://api.cc.bestcodex.app";

const CLAUDE_PLAN_PATTERN = /claude|避风港|cchaven|\bcc\b/i;

export function hasClaudeEntitlement(
  entitlement: Pick<ClaudeEntitlement, "status"> | null | undefined,
): boolean {
  return entitlement?.status === "active" || entitlement?.status === "trialing";
}

export function resolveClaudeEntitlement(input: {
  account: LumioAccountSummary | null;
  remote: ClaudeEntitlement | null;
  local: ClaudeEntitlement | null;
}): ClaudeEntitlement {
  if (input.remote !== null) {
    return { ...input.remote, source: "control-plane" };
  }
  if (input.local !== null) {
    return input.local;
  }
  const label = input.account?.planLabel ?? "";
  if (label !== "" && CLAUDE_PLAN_PATTERN.test(label)) {
    return { status: "active", source: "account" };
  }
  return { status: "none", source: "account" };
}

function asStatus(value: unknown): ClaudeEntitlementStatus | null {
  if (value === "active" || value === "trialing" || value === "none" || value === "expired") {
    return value;
  }
  return null;
}

function readStatus(body: unknown): ClaudeEntitlementStatus | null {
  if (!body || typeof body !== "object") return null;
  const record = body as { status?: unknown; data?: { status?: unknown } };
  return asStatus(record.status) ?? asStatus(record.data?.status);
}

function asOptionalNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function asOptionalString(value: unknown): string | null {
  return typeof value === "string" && value.trim() !== "" ? value : null;
}

function asOptionalBoolean(value: unknown): boolean | null {
  return typeof value === "boolean" ? value : null;
}

function readEntitlementFields(body: unknown): Pick<
  ClaudeEntitlement,
  "expiresAt" | "daysLeft" | "expiringSoon"
> {
  if (!body || typeof body !== "object") {
    return { expiresAt: null, daysLeft: null, expiringSoon: null };
  }
  const record = body as {
    expires_at?: unknown;
    days_left?: unknown;
    expiring_soon?: unknown;
    data?: { expires_at?: unknown; days_left?: unknown; expiring_soon?: unknown };
  };
  const source = record.data ?? record;
  return {
    expiresAt: asOptionalString(source.expires_at),
    daysLeft: asOptionalNumber(source.days_left),
    expiringSoon: asOptionalBoolean(source.expiring_soon),
  };
}

export function entitlementFromSnapshot(
  status: ClaudeEntitlementStatus,
  snapshot: {
    expiresAt?: string | null;
    daysLeft?: number | null;
    expiringSoon?: boolean | null;
  },
  source: ClaudeEntitlement["source"] = "control-plane",
): ClaudeEntitlement {
  const daysLeft = snapshot.daysLeft ?? null;
  return {
    status,
    source,
    expiresAt: snapshot.expiresAt ?? null,
    daysLeft,
    expiringSoon: snapshot.expiringSoon ?? (daysLeft !== null ? daysLeft <= 3 : null),
  };
}

export async function fetchClaudeEntitlementFromControlPlane(
  fetcher: (input: string, init?: RequestInit) => Promise<Response> = fetch,
): Promise<ClaudeEntitlement | null> {
  try {
    const response = await fetcher(`${CLAUDE_CONTROL_API}/api/v1/me/entitlement`);
    if (!response.ok) return null;
    const body: unknown = await response.json();
    const status = readStatus(body);
    if (status === null) return null;
    return entitlementFromSnapshot(status, readEntitlementFields(body));
  } catch {
    return null;
  }
}
