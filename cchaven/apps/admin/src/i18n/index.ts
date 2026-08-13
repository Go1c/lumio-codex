import { zhCN, type MessageKey } from "./zh-CN";

export type Lang = "zh-CN" | "zh-HK";

/**
 * zh-HK 预留位：缺条目自动回落 zh-CN，翻译可以逐条补齐而不会出现空白界面。
 * 后端也支持 zh-HK（Accept-Language），届时错误文案会一并跟随。
 */
const zhHK: Partial<Record<MessageKey, string>> = {};

const DICTS: Record<Lang, Partial<Record<MessageKey, string>>> = {
  "zh-CN": zhCN,
  "zh-HK": zhHK,
};

let currentLang: Lang = "zh-CN";

export function setLang(lang: Lang): void {
  currentLang = lang;
}

export function getLang(): Lang {
  return currentLang;
}

/** t 取文案并做 {name} 插值；缺失条目回落 zh-CN，再缺就原样返回 key，便于发现漏翻。 */
export function t(key: MessageKey, params?: Record<string, string | number>): string {
  const template = DICTS[currentLang][key] ?? zhCN[key] ?? key;
  if (!params) return template;

  return template.replace(/\{(\w+)\}/g, (match, name: string) =>
    name in params ? String(params[name]) : match,
  );
}

export type { MessageKey };
