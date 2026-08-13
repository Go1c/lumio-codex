import { useState } from "react";
import { Link } from "react-router-dom";
import { sleep, useDemo } from "../demo";
import { PasswordField, passwordValid } from "./auth";

interface Session {
  id: string;
  device: string;
  icon: string;
  last: string;
  loc: string;
  current?: boolean;
}

const INITIAL_SESSIONS: Session[] = [
  { id: "s1", device: "Safari · macOS（本浏览器）", icon: "🌐", last: "当前活跃", loc: "上海", current: true },
  { id: "s2", device: "MacBook Pro — CC避风港 APP 1.4.2", icon: "💻", last: "5 分钟前", loc: "上海" },
  { id: "s3", device: "Chrome · Windows", icon: "🌐", last: "2026年8月3日", loc: "杭州" },
];

interface Referral {
  email: string;
  status: "registered" | "activated";
}

const INITIAL_REFERRALS: Referral[] = [
  { email: "w***g@gmail.com", status: "activated" },
  { email: "l***3@qq.com", status: "registered" },
];

export default function SiteAccount() {
  const { authed, setAuthed, email, toast, showErrors, invited } = useDemo();
  const [displayName, setDisplayName] = useState("Mary");
  const [savingName, setSavingName] = useState(false);
  const [curPw, setCurPw] = useState("");
  const [newPw, setNewPw] = useState("");
  const [changingPw, setChangingPw] = useState(false);
  const [sessions, setSessions] = useState(INITIAL_SESSIONS);
  const [revoking, setRevoking] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const referrals = showErrors ? [] : INITIAL_REFERRALS;

  if (!authed) {
    return (
      <div className="acct" style={{ margin: "40px auto" }}>
        <div className="banner warn">
          <span>请先登录后再管理账户。</span>
          <Link to="/login" className="btn btn-primary" style={{ padding: "6px 16px" }}>去登录</Link>
        </div>
      </div>
    );
  }

  async function saveName() {
    setSavingName(true);
    await sleep(600);
    setSavingName(false);
    toast("资料已更新。");
  }

  function copyInvite() {
    setCopied(true);
    toast("已复制邀请链接。");
    setTimeout(() => setCopied(false), 2000);
  }

  async function changePassword(e: React.FormEvent) {
    e.preventDefault();
    setChangingPw(true);
    await sleep(900);
    setChangingPw(false);
    if (showErrors) {
      toast("当前密码不正确。");
      return;
    }
    setCurPw("");
    setNewPw("");
    toast("密码已更新，其他设备已退出登录。");
    setSessions(sessions.filter((s) => s.current));
  }

  async function revoke(s: Session) {
    if (!confirm(`确定退出「${s.device}」的登录吗？`)) return;
    setRevoking(s.id);
    await sleep(700);
    setRevoking(null);
    setSessions(sessions.filter((x) => x.id !== s.id));
    toast("该设备已退出登录。");
  }

  return (
    <div className="acct" style={{ margin: "20px auto 0" }}>
      <h2>账户中心</h2>

      <section>
        <h3>订阅与付款</h3>
        <div style={{ display: "flex", alignItems: "center", gap: 12, marginBottom: 16, flexWrap: "wrap" }}>
          {invited ? (
            <span style={{ background: "#f0fdf4", color: "var(--green)", padding: "5px 14px", borderRadius: 14, fontSize: 13.5, fontWeight: 600 }}>
              免费试用中 · 剩余 23 天（至 2026年9月11日）
            </span>
          ) : (
            <span style={{ background: "#dbeafe", color: "var(--blue)", padding: "5px 14px", borderRadius: 14, fontSize: 13.5, fontWeight: 600 }}>
              已订阅 · 有效期至 2026年9月8日（剩余 27 天）
            </span>
          )}
          <button className="btn btn-primary" onClick={() => toast("原型：跳转支付服务商的安全付款页。")}>
            {invited ? "试用到期前开通订阅" : "续费 / 充值"}
          </button>
        </div>
        <div style={{ fontSize: 13, color: "var(--gray)" }}>
          付款只在本页面完成，APP 内不处理、也不收集任何付款信息。
        </div>
      </section>

      <section>
        <h3>个人资料</h3>
        <div className="field">
          <label>邮箱</label>
          <input value={email} disabled />
        </div>
        <div className="field" style={{ marginBottom: 0 }}>
          <label>显示名称</label>
          <div style={{ display: "flex", gap: 8 }}>
            <input value={displayName} onChange={(e) => setDisplayName(e.target.value)} />
            <button className="btn btn-secondary" onClick={saveName} disabled={savingName || !displayName.trim()}>
              {savingName ? "保存中…" : "保存"}
            </button>
          </div>
        </div>
      </section>

      <section>
        <h3>邀请好友</h3>
        <p style={{ fontSize: 13.5, color: "var(--gray)", marginBottom: 12 }}>
          朋友经你的链接下载、注册并登录 APP 后，将获得首月免费试用（每个账号限一次）；
          每成功邀请 1 人，你的订阅延长 7 天。
        </p>
        {referrals.some((r) => r.status === "activated") && (
          <div className="banner ok" style={{ marginBottom: 14 }}>
            🎉 已成功邀请 {referrals.filter((r) => r.status === "activated").length} 人 · 订阅共延长{" "}
            {referrals.filter((r) => r.status === "activated").length * 7} 天
          </div>
        )}
        <div style={{ display: "flex", gap: 8, marginBottom: 16 }}>
          <input
            value="https://cchaven.cn/i/mary8k2f"
            readOnly
            style={{ flex: 1, padding: "9px 12px", border: "1px solid #d3d7de", borderRadius: 8, fontFamily: "monospace", fontSize: 13, background: "#fafbfc" }}
          />
          <button className="btn btn-primary" onClick={copyInvite} disabled={copied}>
            {copied ? "已复制 ✓" : "复制链接"}
          </button>
        </div>
        {referrals.length === 0 ? (
          <div style={{ fontSize: 13.5, color: "var(--gray)", padding: "8px 0" }}>还没有朋友加入。</div>
        ) : (
          referrals.map((r) => (
            <div className="sess-row" key={r.email}>
              <span style={{ fontSize: 18 }}>👤</span>
              <div>
                <div style={{ fontFamily: "monospace", fontSize: 13.5 }}>{r.email}</div>
                <div className="meta">
                  {r.status === "activated" ? "已注册并登录 ✓ 试用已发放 · 你的订阅 +7 天" : "已注册，尚未登录 APP"}
                </div>
              </div>
            </div>
          ))
        )}
      </section>

      <section>
        <h3>安全</h3>
        <form onSubmit={changePassword} style={{ maxWidth: 380 }}>
          <div className="field">
            <label>当前密码</label>
            <input type="password" value={curPw} onChange={(e) => setCurPw(e.target.value)} />
          </div>
          <PasswordField label="新密码" value={newPw} onChange={setNewPw} />
          <button className="btn btn-primary" disabled={!curPw || !passwordValid(newPw) || changingPw}>
            {changingPw && <span className="spinner" />}
            {changingPw ? "更新中…" : "修改密码"}
          </button>
        </form>
        <p style={{ fontSize: 13, color: "var(--gray)", marginTop: 14 }}>
          修改邮箱：需要先验证新邮箱，原邮箱会收到通知（原型未展开）。
        </p>
      </section>

      <section>
        <h3>登录设备与授权</h3>
        <p style={{ fontSize: 13, color: "var(--gray)", marginBottom: 10 }}>
          包括通过浏览器授权登录的 CC避风港 APP。在这里退出即可撤销该设备的授权。
        </p>
        {sessions.map((s) => (
          <div className="sess-row" key={s.id}>
            <span style={{ fontSize: 20 }}>{s.icon}</span>
            <div>
              <div>
                {s.device} {s.current && <span className="this">本设备</span>}
              </div>
              <div className="meta">{s.last} · {s.loc}</div>
            </div>
            {!s.current && (
              <button className="btn btn-secondary" onClick={() => revoke(s)} disabled={revoking === s.id}>
                {revoking === s.id ? "退出中…" : "退出该设备"}
              </button>
            )}
          </div>
        ))}
        {sessions.length > 1 && (
          <button
            className="btn btn-secondary"
            style={{ marginTop: 14 }}
            onClick={() => {
              if (!confirm("确定退出所有其他设备的登录吗？")) return;
              setSessions(sessions.filter((s) => s.current));
              toast("所有其他设备已退出登录。");
            }}
          >
            退出所有其他设备
          </button>
        )}
      </section>

      <section className="danger-zone">
        <h3>危险操作</h3>
        <div style={{ display: "flex", gap: 12 }}>
          <button className="btn btn-secondary" onClick={() => { setAuthed(false); toast("已退出登录。"); }}>退出登录</button>
          <button className="btn btn-danger" onClick={() => toast("原型：注销账号有 7 天冷静期，期间可撤销。")}>
            注销账号…
          </button>
        </div>
      </section>
    </div>
  );
}
