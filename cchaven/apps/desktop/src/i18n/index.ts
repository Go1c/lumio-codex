import { zhCN, type Dictionary } from "./zh-CN";
import { zhHK, type DeepPartial } from "./zh-HK";

export type Locale = "zh-CN" | "zh-HK";

const DICTIONARIES: Record<Locale, DeepPartial<Dictionary>> = {
  "zh-CN": zhCN,
  "zh-HK": zhHK,
};

let activeLocale: Locale = "zh-CN";

export function setLocale(locale: Locale): void {
  activeLocale = locale;
}

export function getLocale(): Locale {
  return activeLocale;
}

function lookup(dictionary: unknown, path: string[]): unknown {
  return path.reduce<unknown>((node, key) => {
    if (node && typeof node === "object" && key in (node as Record<string, unknown>)) {
      return (node as Record<string, unknown>)[key];
    }
    return undefined;
  }, dictionary);
}

/**
 * Translate a dotted key, falling back to zh-CN for any key a locale has not
 * translated yet. Placeholders are written `{name}`.
 */
export function t(key: string, vars?: Record<string, string | number>): string {
  const path = key.split(".");
  const value = lookup(DICTIONARIES[activeLocale], path) ?? lookup(zhCN, path);
  if (typeof value !== "string") {
    // Surfacing the key beats rendering "undefined" in front of a user.
    return key;
  }
  if (!vars) return value;
  return value.replace(/\{(\w+)\}/g, (match, name: string) =>
    name in vars ? String(vars[name]) : match,
  );
}

/** Array-valued entries such as the wizard step names. */
export function tList(key: string): readonly string[] {
  const path = key.split(".");
  const value = lookup(DICTIONARIES[activeLocale], path) ?? lookup(zhCN, path);
  return Array.isArray(value) ? (value as readonly string[]) : [];
}
