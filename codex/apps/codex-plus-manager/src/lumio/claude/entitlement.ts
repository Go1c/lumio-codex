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

export async function fetchClaudeEntitlementFromControlPlane(
  fetcher: (input: string, init?: RequestInit) => Promise<Response> = fetch,
): Promise<ClaudeEntitlement | null> {
  try {
    const response = await fetcher(`${CLAUDE_CONTROL_API}/api/v1/me/entitlement`);
    if (!response.ok) return null;
    const status = readStatus(await response.json());
    if (status === null) return null;
    return { status, source: "control-plane" };
  } catch {
    return null;
  }
}
