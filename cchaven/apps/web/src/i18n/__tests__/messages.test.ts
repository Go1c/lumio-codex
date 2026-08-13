import { describe, expect, it } from "vitest";

import { t } from "@/i18n";
import { zhCN } from "@/i18n/zh-CN";

/**
 * 交互设计 6.2 节「防枚举与限频文案」是安全语义文案，必须逐字一致。
 * 这个测试与服务端 internal/i18n/i18n_test.go 互为镜像：任何一边改字都会红。
 */
describe("6.2 节固定文案", () => {
  const FIXED_MESSAGES: Array<[keyof typeof zhCN, string]> = [
    ["auth.login_failed", "邮箱或密码不正确。"],
    ["auth.forgot_password_submitted", "如 {email} 已注册账号，你将很快收到重设链接。"],
    ["auth.code_invalid", "验证码不正确，还剩 {n} 次尝试机会。"],
    ["auth.code_expired", "该验证码已过期，请重新发送。"],
    ["common.rate_limited", "尝试次数过多，请 {n} {unit}后再试。"],
    ["auth.session_expired", "登录已过期，请重新登录。"],
    ["billing.trial_already_used", "每个账号只可享用一次免费试用。"],
  ];

  it.each(FIXED_MESSAGES)("%s 逐字正确", (key, expected) => {
    expect(zhCN[key]).toBe(expected);
  });

  it("七条一条不少", () => {
    expect(FIXED_MESSAGES).toHaveLength(7);
  });

  it("插值后与服务端渲染结果一致", () => {
    expect(t("auth.forgot_password_submitted", { email: "mary@example.com" })).toBe(
      "如 mary@example.com 已注册账号，你将很快收到重设链接。",
    );
    expect(t("auth.code_invalid", { n: 3 })).toBe("验证码不正确，还剩 3 次尝试机会。");
    expect(t("common.rate_limited", { n: 1, unit: "分钟" })).toBe("尝试次数过多，请 1 分钟后再试。");
  });

  it("zh-HK 缺失词条回落 zh-CN", () => {
    expect(t("auth.login_failed", undefined, "zh-HK")).toBe("邮箱或密码不正确。");
  });
});
