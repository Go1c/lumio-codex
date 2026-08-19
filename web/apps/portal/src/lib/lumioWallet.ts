/**
 * Sub2API 用户余额流水。只用用户 JWT，禁止带 bcs_ 消费方密钥。
 * 流水不是 Claude 月卡；套餐时长仍以控制面 entitlement 为准。
 */

import { apiBaseUrl } from "@lumio/ui/config";

export interface BalanceTransaction {
  purpose: string;
  ref: string;
  amount: number;
  createdAt: string;
}

const UNAVAILABLE = "暂时无法读取余额流水。";

export class WalletError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "WalletError";
  }
}

type Json = Record<string, unknown>;

function str(value: unknown, fallback = ""): string {
  return typeof value === "string" ? value : fallback;
}

function num(value: unknown, fallback = 0): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

export async function fetchBalanceTransactions(accessToken: string): Promise<BalanceTransaction[]> {
  let response: Response;
  try {
    response = await fetch(`${apiBaseUrl()}/api/v1/user/balance/transactions`, {
      method: "GET",
      headers: {
        Accept: "application/json",
        Authorization: `Bearer ${accessToken}`,
      },
    });
  } catch {
    throw new WalletError(UNAVAILABLE);
  }

  const text = await response.text().catch(() => "");
  let body: Json | null = null;
  try {
    body = JSON.parse(text) as Json;
  } catch {
    body = null;
  }
  if (!response.ok || !body || body.code !== 0) {
    throw new WalletError(str(body?.message, UNAVAILABLE) || UNAVAILABLE);
  }
  const data = body.data;
  const rawItems = Array.isArray(data)
    ? data
    : data && typeof data === "object"
      ? ((data as Json).items ?? (data as Json).list ?? [])
      : [];
  const items = Array.isArray(rawItems) ? rawItems : [];
  return items.map((item) => {
    const row = (item ?? {}) as Json;
    return {
      purpose: str(row.purpose),
      ref: str(row.ref),
      amount: num(row.amount),
      createdAt: str(row.created_at) || str(row.createdAt),
    };
  });
}
