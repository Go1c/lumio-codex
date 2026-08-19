import { useState } from "react";
import { Link, useLocation, useNavigate } from "react-router-dom";

import { Banner, LoadingBlock, SectionCard, StatusDot, purchaseUrl, siteUrl } from "@lumio/ui";

import { AffiliateCard } from "@/components/AffiliateCard";
import {
  BalanceTransactionsCard,
  ClaudeOrdersCard,
  ClaudeSubscriptionCard,
} from "@/components/ClaudeAccountPanels";
import {
  ACCOUNT_TABS,
  accountTabFromHash,
  hashForAccountTab,
  type AccountTabId,
} from "@/lib/accountTabs";
import { usePortalSession } from "@/state/session";

const STATUS_TONE: Record<string, { tone: "green" | "orange" | "gray"; label: string }> = {
  active: { tone: "green", label: "正常" },
  disabled: { tone: "gray", label: "已停用" },
  pending: { tone: "orange", label: "待激活" },
};

export function Account() {
  const session = usePortalSession();
  const location = useLocation();
  const navigate = useNavigate();
  const tab = accountTabFromHash(location.hash);
  const [billingTick, setBillingTick] = useState(0);

  function selectTab(next: AccountTabId): void {
    navigate(
      { pathname: location.pathname, search: location.search, hash: hashForAccountTab(next) },
      { replace: true },
    );
  }

  if (session.status === "anonymous") {
    return (
      <div className="auth-page">
        <div className="auth-card">
          <h2>账户中心</h2>
          <p className="sub">请先登录后再查看账户信息。</p>
          <Link to="/login" className="btn btn-primary btn-block">
            去登录
          </Link>
        </div>
      </div>
    );
  }

  const profile = session.profile;

  return (
    <div className="acct">
      <h2>账户中心</h2>
      {!profile ? (
        <LoadingBlock label="读取账户信息…" lines={4} />
      ) : (
        <>
          <div className="acct-tabs" role="tablist" aria-label="账户中心">
            {ACCOUNT_TABS.map((item) => (
              <button
                key={item.id}
                type="button"
                role="tab"
                id={`acct-tab-${item.id}`}
                aria-controls={`acct-panel-${item.id}`}
                aria-selected={tab === item.id}
                className="acct-tab"
                onClick={() => selectTab(item.id)}
              >
                {item.label}
              </button>
            ))}
          </div>

          <div
            role="tabpanel"
            id={`acct-panel-${tab}`}
            aria-labelledby={`acct-tab-${tab}`}
          >
            {tab === "profile" ? (
              <>
                <SectionCard title="账号" id="profile">
                  <p className="profile-label">邮箱</p>
                  <p className="readonly-value">{profile.email}</p>
                  <hr className="section-divider" />
                  <p className="profile-label">状态</p>
                  <StatusDot
                    tone={STATUS_TONE[profile.status]?.tone ?? "gray"}
                    label={STATUS_TONE[profile.status]?.label ?? profile.status ?? "未知"}
                  />
                </SectionCard>

                <SectionCard title="在哪里使用" id="products">
                  <p className="note">
                    同一个账号用于 <a href={siteUrl("codex")}>BestCodex</a>
                    。一个启动器，一次下载，无需分别注册。
                  </p>
                </SectionCard>

                <Banner kind="info" action={<LogoutButton />}>
                  退出后本机三站的登录状态会一并清除。
                </Banner>
              </>
            ) : null}

            {tab === "balance" ? (
              <>
                <SectionCard title="余额与充值" id="balance">
                  <div className="sub-row">
                    <span className="price mono">{formatBalance(profile.balance)}</span>
                    <a
                      className="btn btn-primary"
                      href={purchaseUrl(session.accessToken)}
                      target="_blank"
                      rel="noreferrer"
                    >
                      充值
                    </a>
                  </div>
                  <p className="note">
                    充值在收银台完成，BestCodex 官网与客户端都不收集任何付款信息。
                  </p>
                </SectionCard>
                {session.accessToken ? (
                  <BalanceTransactionsCard accessToken={session.accessToken} />
                ) : null}
              </>
            ) : null}

            {tab === "orders" && session.accessToken ? (
              <>
                <ClaudeSubscriptionCard
                  accessToken={session.accessToken}
                  reloadKey={billingTick}
                  onBillingChanged={() => setBillingTick((n) => n + 1)}
                />
                <ClaudeOrdersCard
                  accessToken={session.accessToken}
                  reloadKey={billingTick}
                  onBillingChanged={() => setBillingTick((n) => n + 1)}
                />
              </>
            ) : null}

            {tab === "affiliate" ? (
              <AffiliateCard
                accessToken={session.accessToken}
                onBalanceChanged={session.reload}
              />
            ) : null}
          </div>
        </>
      )}
    </div>
  );
}

function LogoutButton() {
  const session = usePortalSession();
  return (
    <button type="button" className="btn btn-secondary" onClick={() => void session.signOut()}>
      退出登录
    </button>
  );
}

/** Sub2API 的余额是以元为单位的浮点数。 */
export function formatBalance(balance: number): string {
  return `¥${balance.toFixed(2)}`;
}
