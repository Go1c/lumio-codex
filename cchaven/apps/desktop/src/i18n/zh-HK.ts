import type { Dictionary } from "./zh-CN";

/**
 * 繁體中文（香港）— 6.5 節要求預留的語言槽位。
 *
 * 只覆寫已翻譯的鍵，其餘由 zh-CN 兜底（見 `i18n/index.ts` 的 `resolve`），
 * 因此這份字典可以逐步補齊而不會出現空白介面。
 */
export const zhHK: DeepPartial<Dictionary> = {
  brand: {
    name: "CC避風港",
  },
  common: {
    cancel: "取消",
    retry: "重試",
    confirm: "確定",
    delete: "刪除",
    undo: "撤銷",
  },
  fixed: {
    sessionExpired: "登入已過期，請重新登入。",
    trialReuse: "每個帳號只可享用一次免費試用。",
  },
  login: {
    title: "登入",
    button: "透過瀏覽器登入 ↗",
  },
};

/** Widens the literal types `as const` gives zh-CN, so translations are free text. */
export type DeepPartial<T> = {
  [K in keyof T]?: T[K] extends string
    ? string
    : T[K] extends readonly string[]
      ? readonly string[]
      : T[K] extends object
        ? DeepPartial<T[K]>
        : T[K];
};
