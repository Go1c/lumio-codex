import type { MessageKey } from "./zh-CN";

/**
 * 繁体中文（香港）词条位（交互设计 6.5 节要求预留）。
 *
 * 目前整体回落到 zh-CN：只需在此补条目即可逐条覆盖，无需改动调用处。
 * 注意：6.2 节的安全语义文案要与服务端 i18n 的 zh-HK 词条同步补充，避免两端不一致。
 */
export const zhHK: Partial<Record<MessageKey, string>> = {};
