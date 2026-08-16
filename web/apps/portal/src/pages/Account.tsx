import { Link } from "react-router-dom";

import { Banner, LoadingBlock, SectionCard, StatusDot, purchaseUrl, siteUrl } from "@lumio/ui";

import { AffiliateCard } from "@/components/AffiliateCard";
import { usePortalSession } from "@/state/session";

const STATUS_TONE: Record<string, { tone: "green" | "orange" | "gray"; label: string }> = {
  active: { tone: "green", label: "正常" },
  disabled: { tone: "gray", label: "已停用" },
  pending: { tone: "orange", label: "待激活" },
};

export function Account() {
  const session = usePortalSession();

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

          <SectionCard title="余额与充值" id="balance">
            <div className="sub-row">
              <span className="price mono">{formatBalance(profile.balance)}</span>
              <a
                className="btn btn-primary"
                href={purchaseUrl()}
                target="_blank"
                rel="noreferrer"
              >
                充值
              </a>
            </div>
            <p className="note">
              充值在 Sub2API 收银台完成，Lumio 官网与客户端都不收集任何付款信息。
            </p>
          </SectionCard>

          <AffiliateCard
            accessToken={session.accessToken}
            onBalanceChanged={session.reload}
          />

          <SectionCard title="在哪里使用" id="products">
            <p className="note">
              同一个账号可直接用于 <a href={siteUrl("codex")}>Lumio Codex</a> 与{" "}
              <a href={siteUrl("cc")}>CC避风港</a>，无需分别注册。
            </p>
          </SectionCard>

          <Banner kind="info" action={<LogoutButton />}>
            退出后本机三站的登录状态会一并清除。
          </Banner>
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
