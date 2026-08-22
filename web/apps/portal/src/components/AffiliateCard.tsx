import { useEffect, useState } from "react";

import {
  fetchAffiliate,
  transferAffiliateQuota,
  type AffiliateDetail,
  type AffiliateRuntimeRules,
} from "@lumio/auth";
import { Banner, ErrorBlock, LoadingBlock, SectionCard } from "@lumio/ui";

import { messageOf } from "@/lib/authFlow";
import { affiliateInviteLink } from "@/lib/affiliateRef";

import { formatBalance } from "@/pages/Account";

/**
 * 账户中心的「邀请返利」卡片。所有数值（额度、比例、阶梯、运行规则）都来自
 * `GET /user/aff` 的实时响应——后台改配置这里自动跟上，前端不存任何业务数值。
 * `rules` 为 null 时（后端尚未部署 sub2api PR #307）隐藏对应文案，而不是当作 0。
 */
export function AffiliateCard({
  accessToken,
  onBalanceChanged,
}: {
  accessToken?: string;
  onBalanceChanged: () => void;
}) {
  const [detail, setDetail] = useState<AffiliateDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const [nonce, setNonce] = useState(0);

  useEffect(() => {
    if (!accessToken) return;
    let cancelled = false;
    setError(null);
    fetchAffiliate(accessToken)
      .then((result) => {
        if (!cancelled) setDetail(result);
      })
      .catch((failure) => {
        if (!cancelled) setError(messageOf(failure));
      });
    return () => {
      cancelled = true;
    };
  }, [accessToken, nonce]);

  if (!accessToken) return null;
  if (error) {
    return (
      <SectionCard title="邀请返利" id="affiliate">
        <ErrorBlock message={error} onRetry={() => setNonce((n) => n + 1)} />
      </SectionCard>
    );
  }
  if (!detail) {
    return (
      <SectionCard title="邀请返利" id="affiliate">
        <LoadingBlock label="读取邀请返利…" lines={3} />
      </SectionCard>
    );
  }

  // 守卫之后捕获非空快照：transfer/copyInviteLink 是闭包，不能依赖上面的收窄。
  const affiliate = detail;

  async function transfer(): Promise<void> {
    if (!accessToken || busy) return;
    setBusy(true);
    setNotice(null);
    try {
      const result = await transferAffiliateQuota(accessToken);
      setNotice(`已划转 ${formatBalance(result.transferredQuota)} 到账户余额。`);
      onBalanceChanged();
      setNonce((n) => n + 1);
    } catch (failure) {
      setNotice(messageOf(failure));
    } finally {
      setBusy(false);
    }
  }

  async function copyInviteLink(): Promise<void> {
    try {
      await navigator.clipboard.writeText(
        affiliateInviteLink(window.location.origin, affiliate.affCode),
      );
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      setNotice("复制失败，请手动复制邀请码。");
    }
  }

  return (
    <SectionCard title="邀请返利" id="affiliate">
      {notice && <Banner kind="info">{notice}</Banner>}
      <div className="sub-row">
        <div>
          <p className="profile-label">我的邀请码</p>
          <p className="price mono">{detail.affCode}</p>
        </div>
        <button type="button" className="btn btn-secondary" onClick={() => void copyInviteLink()}>
          {copied ? "已复制" : "复制邀请链接"}
        </button>
      </div>
      <p className="note">
        好友通过你的邀请链接注册并充值，你即可获得返利
        {rateText(detail.effectiveRebateRatePercent)}。
        {rulesText(detail.rules)}
      </p>
      <hr className="section-divider" />

      <div className="sub-row">
        <div>
          <p className="profile-label">可划转返利</p>
          <p className="price mono">{formatBalance(detail.affQuota)}</p>
        </div>
        <button
          type="button"
          className="btn btn-primary"
          disabled={busy || detail.affQuota <= 0}
          onClick={() => void transfer()}
        >
          划转到余额
        </button>
      </div>
      {detail.affFrozenQuota > 0 && (
        <p className="note">另有 {formatBalance(detail.affFrozenQuota)} 冻结中，到期自动解锁。</p>
      )}
      <p className="note">
        累计获得 {formatBalance(detail.affHistoryQuota)}；已邀请 {detail.affCount} 人
        {detail.nextTier
          ? nextTierText(detail.nextTier, detail.affCount, detail.inviteeRechargeTotal)
          : ""}。
      </p>

      {detail.invitees.length > 0 && (
        <>
          <hr className="section-divider" />
          <p className="profile-label">通过我注册的好友（{detail.invitees.length}）</p>
          <ul className="plain-list">
            {detail.invitees.map((invitee) => (
              <li key={invitee.userId} className="mono">
                {invitee.email}
                <span className="note"> · {formatDay(invitee.createdAt)}</span>
              </li>
            ))}
          </ul>
        </>
      )}
    </SectionCard>
  );
}

function rateText(rate: number | null): string {
  return rate === null ? "" : `（当前比例 ${rate}%）`;
}

/** 运行规则文案只由接口 `rules` 驱动；字段缺失（旧后端）时整段隐藏。 */
function rulesText(rules: AffiliateRuntimeRules | null): string {
  if (!rules) return "";
  const parts: string[] = [];
  if (rules.signupBonusEnabled && rules.signupBonusAmount > 0) {
    parts.push(`好友完成注册即得 ${formatBalance(rules.signupBonusAmount)}`);
  }
  if (rules.rebateFreezeHours > 0) {
    parts.push(`返利冻结 ${rules.rebateFreezeHours} 小时后可划转`);
  }
  if (rules.rebateDurationDays > 0) {
    parts.push(`好友注册后 ${rules.rebateDurationDays} 天内的充值计入返利`);
  }
  if (rules.rebatePerInviteeCap > 0) {
    parts.push(`每位好友返利上限 ${formatBalance(rules.rebatePerInviteeCap)}`);
  }
  return parts.length > 0 ? parts.join("；") + "。" : "";
}

function nextTierText(
  tier: AffiliateDetail["nextTier"],
  affCount: number,
  inviteeRechargeTotal: number,
): string {
  if (!tier) return "";
  const needPeople = Math.max(tier.minInvitees - affCount, 0);
  const needRecharge = Math.max(tier.minRecharge - inviteeRechargeTotal, 0);
  const rate = tier.rebateRatePercent === null ? "" : `（比例升至 ${tier.rebateRatePercent}%）`;
  if (needPeople <= 0 && needRecharge <= 0) {
    return `；即将升至 ${tier.level}${rate}`;
  }
  const parts: string[] = [];
  if (needPeople > 0) parts.push(`再邀请 ${needPeople} 人`);
  if (needRecharge > 0) parts.push(`好友再累计充值 ${formatBalance(needRecharge)}`);
  return `；${parts.join("、")}升至 ${tier.level}${rate}`;
}

function formatDay(iso: string): string {
  const day = new Date(iso);
  return Number.isNaN(day.getTime()) ? "" : day.toLocaleDateString("zh-CN");
}
