/** 交互设计 6.5 节：日期一律 `YYYY年M月D日`。 */
export function formatDate(input: string | Date | undefined | null): string {
  if (!input) return "—";
  const date = input instanceof Date ? input : new Date(input);
  if (Number.isNaN(date.getTime())) return "—";
  return `${date.getFullYear()}年${date.getMonth() + 1}月${date.getDate()}日`;
}

const CURRENCY_SYMBOLS: Record<string, string> = {
  CNY: "¥",
  USD: "$",
  HKD: "HK$",
};

/**
 * 金额格式化。币种与数值都由后台下发，页面不写死（6.5 节）。
 * 整数金额省略小数位：6800 分 → ¥68。
 */
export function formatMoney(amountCents: number, currency: string): string {
  const symbol = CURRENCY_SYMBOLS[currency] ?? `${currency} `;
  const amount = amountCents / 100;
  const text = Number.isInteger(amount) ? String(amount) : amount.toFixed(2);
  return `${symbol}${text}`;
}

/** 相对时间，用于设备列表「最近活跃」。超过 7 天回落到日期。 */
export function formatRelativeTime(input: string | Date, now: Date = new Date()): string {
  const date = input instanceof Date ? input : new Date(input);
  if (Number.isNaN(date.getTime())) return "—";

  const diffSeconds = Math.floor((now.getTime() - date.getTime()) / 1000);
  if (diffSeconds < 60) return "刚刚";
  if (diffSeconds < 3600) return `${Math.floor(diffSeconds / 60)} 分钟前`;
  if (diffSeconds < 86400) return `${Math.floor(diffSeconds / 3600)} 小时前`;
  if (diffSeconds < 7 * 86400) return `${Math.floor(diffSeconds / 86400)} 天前`;
  return formatDate(date);
}

/**
 * 6.6 节：长邮箱 / 长路径中间省略号截断，完整内容由调用方放进 title 供 hover 查看。
 */
export function middleEllipsis(text: string, max = 28): string {
  if (text.length <= max) return text;
  const head = Math.ceil((max - 1) / 2);
  const tail = Math.floor((max - 1) / 2);
  return `${text.slice(0, head)}…${text.slice(text.length - tail)}`;
}

export function maskEmailLocalPart(email: string): string {
  const [local, domain] = email.split("@");
  if (!local || !domain) return "***";
  if (local.length < 3) return `${local[0]}***@${domain}`;
  return `${local[0]}***${local[local.length - 1]}@${domain}`;
}

const CHANNEL_LABELS: Record<string, string> = {
  alipay: "支付宝",
  wechat: "微信支付",
  card: "银行卡",
  mock: "测试通道（mock）",
};

export function channelLabel(channel: string): string {
  return CHANNEL_LABELS[channel] ?? channel;
}
