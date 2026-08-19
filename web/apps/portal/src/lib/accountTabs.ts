export const ACCOUNT_TABS = [
  { id: "profile", label: "账户" },
  { id: "balance", label: "余额" },
  { id: "orders", label: "开通记录" },
  { id: "affiliate", label: "邀请返利" },
] as const;

export type AccountTabId = (typeof ACCOUNT_TABS)[number]["id"];

const HASH_TO_TAB: Record<string, AccountTabId> = {
  "": "profile",
  profile: "profile",
  account: "profile",
  balance: "balance",
  "balance-transactions": "balance",
  orders: "orders",
  "claude-orders": "orders",
  "claude-subscription": "orders",
  affiliate: "affiliate",
};

export function accountTabFromHash(hash: string): AccountTabId {
  const key = hash.replace(/^#/, "").trim().toLowerCase();
  return HASH_TO_TAB[key] ?? "profile";
}

/** 账户页签不写 hash，其余页签用 `#id`，方便桌面「开通记录」深链。 */
export function hashForAccountTab(tab: AccountTabId): string {
  return tab === "profile" ? "" : tab;
}
