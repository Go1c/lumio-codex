import { describe, expect, it } from "vitest";
import { formatBytes, formatDate, formatRelative, truncateMiddle } from "./format";
import { t } from "../i18n";
import { backoffFor } from "../components/TerminalPane";

describe("格式化（6.5 / 6.6）", () => {
  it("日期一律 YYYY年M月D日，月份日期不补零", () => {
    expect(formatDate("2026-09-08T00:00:00Z")).toBe("2026年9月8日");
    expect(formatDate("2026-12-25T10:00:00Z")).toBe("2026年12月25日");
    expect(formatDate(null)).toBe("");
    expect(formatDate("not a date")).toBe("");
  });

  it("相对时间按界面实际粒度收敛", () => {
    const now = Date.UTC(2026, 7, 12, 12, 0, 0);
    expect(formatRelative(now - 30_000, now)).toBe("刚刚");
    expect(formatRelative(now - 2 * 60_000, now)).toBe("2 分钟前");
    expect(formatRelative(now - 3 * 3_600_000, now)).toBe("3 小时前");
    expect(formatRelative(now - 26 * 3_600_000, now)).toBe("昨天");
    expect(formatRelative(now - 3 * 86_400_000, now)).toBe("3 天前");
  });

  it("长文本中间省略，便于 hover 显示全文", () => {
    expect(truncateMiddle("short", 32)).toBe("short");
    const long = "a-very-long-project-name-that-overflows-the-sidebar";
    const truncated = truncateMiddle(long, 20);
    expect(truncated).toHaveLength(20);
    expect(truncated).toContain("…");
  });

  it("文件大小按 KB/MB 显示", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(18_842)).toBe("18.4 KB");
    expect(formatBytes(2_000_000)).toBe("1.9 MB");
  });
});

describe("i18n 字典", () => {
  it("占位符按名替换", () => {
    expect(t("status.conflicts", { n: 2 })).toBe("2 个冲突");
    expect(t("account.trialing", { n: 23 })).toBe("免费试用中 · 剩余 23 天");
  });

  it("6.2 固定文案逐字保留", () => {
    expect(t("fixed.sessionExpired")).toBe("登录已过期，请重新登录。");
    expect(t("fixed.trialReuse")).toBe("每个账号只可享用一次免费试用。");
  });

  it("未知键返回键名而不是 undefined", () => {
    expect(t("nope.missing")).toBe("nope.missing");
  });
});

describe("重连退避（6.3）", () => {
  it("按 2s→5s→10s→30s 递增并封顶", () => {
    expect([0, 1, 2, 3, 4, 9].map(backoffFor)).toEqual([2, 5, 10, 30, 30, 30]);
  });
});
