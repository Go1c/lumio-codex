import { createContext, useContext, useMemo, type ReactNode } from "react";

import { zhCN, type MessageKey } from "./zh-CN";
import { zhHK } from "./zh-HK";

export type Lang = "zh-CN" | "zh-HK";

export const DEFAULT_LANG: Lang = "zh-CN";

const dictionaries: Record<Lang, Partial<Record<MessageKey, string>>> = {
  "zh-CN": zhCN,
  "zh-HK": zhHK,
};

export type MessageParams = Record<string, string | number>;

/** 渲染文案，`{key}` 形式插值；缺失词条回落 zh-CN，仍缺失则返回 key 本身以便测试暴露遗漏。 */
export function t(key: MessageKey, params?: MessageParams, lang: Lang = DEFAULT_LANG): string {
  const template = dictionaries[lang]?.[key] ?? zhCN[key] ?? key;
  if (!params) return template;

  return Object.entries(params).reduce(
    (acc, [name, value]) => acc.replaceAll(`{${name}}`, String(value)),
    template,
  );
}

interface LangContextValue {
  lang: Lang;
  t: (key: MessageKey, params?: MessageParams) => string;
}

const LangContext = createContext<LangContextValue>({
  lang: DEFAULT_LANG,
  t: (key, params) => t(key, params, DEFAULT_LANG),
});

export function LangProvider({ lang = DEFAULT_LANG, children }: { lang?: Lang; children: ReactNode }) {
  const value = useMemo<LangContextValue>(
    () => ({ lang, t: (key, params) => t(key, params, lang) }),
    [lang],
  );
  return <LangContext.Provider value={value}>{children}</LangContext.Provider>;
}

export function useT() {
  return useContext(LangContext).t;
}

export function useLang() {
  return useContext(LangContext).lang;
}

export type { MessageKey };
