import { useState } from "react";

import { getReferrals } from "@/api/endpoints";
import type { ReferralOverview } from "@/api/types";
import { useToast } from "@/components/Toast";
import { Banner, EmptyBlock, ErrorBlock, LoadingBlock, SectionCard, Truncated } from "@/components/ui";
import { useResource } from "@/hooks/useResource";
import { useT } from "@/i18n";
import { formatDate } from "@/lib/format";

/**
 * 5.6「邀请好友」。
 *
 * 奖励天数一律取服务端下发的 `reward_days`：为 0 时（后台关闭奖励）隐藏所有
 * 「订阅延长 X 天」相关文案与汇总行。
 */
export function ReferralSection() {
  const t = useT();
  const toast = useToast();
  const { status, data, error, reload } = useResource<ReferralOverview>(
    (signal) => getReferrals(signal),
    [],
  );
  const [copied, setCopied] = useState(false);

  async function copyLink(link: string) {
    try {
      await navigator.clipboard?.writeText(link);
    } catch {
      // 剪贴板不可用时仍给出反馈，用户可以手动选中输入框复制。
    }
    setCopied(true);
    toast(t("account.referral_copied"));
    setTimeout(() => setCopied(false), 2000);
  }

  return (
    <SectionCard id="referral" title={t("account.referral")}>
      {status === "loading" && <LoadingBlock lines={3} />}

      {status === "error" && (
        <ErrorBlock error={error} fallback={t("account.load_error")} onRetry={reload} />
      )}

      {status === "success" && data && (
        <>
          <p className="note" style={{ marginBottom: 12 }}>
            {data.reward_days > 0
              ? t("account.referral_note_full", { n: data.reward_days })
              : t("account.referral_note_short")}
          </p>

          {data.invited_count > 0 && (
            <Banner kind="ok">
              {data.reward_days > 0
                ? t("account.referral_summary", {
                    n: data.invited_count,
                    days: data.total_bonus_days,
                  })
                : t("account.referral_summary_no_bonus", { n: data.invited_count })}
            </Banner>
          )}

          <div className="invite-link-row">
            <label className="sr-only" htmlFor="invite-link">
              {t("account.referral_link_label")}
            </label>
            <input id="invite-link" value={data.link} readOnly onFocus={(e) => e.target.select()} />
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => void copyLink(data.link)}
              disabled={copied}
            >
              {copied ? t("common.copied") : t("common.copy")}
            </button>
          </div>

          {data.items.length === 0 ? (
            <EmptyBlock icon="👥" text={t("account.referral_empty")} />
          ) : (
            <ul className="referral-list">
              {data.items.map((item) => (
                <li className="sess-row" key={`${item.email_masked}-${item.at}`}>
                  <span aria-hidden="true">👤</span>
                  <div className="grow">
                    <div className="mono">
                      <Truncated text={item.email_masked} max={30} />
                    </div>
                    <div className="meta">
                      {item.status === "activated"
                        ? item.bonus_days > 0
                          ? t("account.referral_activated_bonus", { n: item.bonus_days })
                          : t("account.referral_activated")
                        : t("account.referral_registered")}
                      {" · "}
                      {formatDate(item.at)}
                    </div>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </SectionCard>
  );
}
