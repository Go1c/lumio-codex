import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useDemo } from "../demo";

/* ---------- 首页（精简版：防封 + 双向同步 两件事） ---------- */

const TERM_SHOT = [
  "$ ssh dev-server",
  "Attached to session cc-my-project",
  "",
  "> claude",
  "╭──────────────────────────────────────────╮",
  "│  Claude Code · my-project                │",
  "│  How can I help you today?               │",
  "╰──────────────────────────────────────────╯",
  "",
  "> Refactor the sync engine to batch writes_",
];

export function Home() {
  const { invited } = useDemo();
  return (
    <>
      <section className="hero">
        <h1>
          安心使用 Claude Code
          <br />
          不再担心封号
        </h1>
        <p className="sub">
          Claude Code 运行在你自己的服务器上——独立环境、固定 IP、持久会话；
          文件双向安全同步，编辑体验如同本机。
        </p>
        <div className="ctas">
          <Link to="/download" className="btn btn-primary btn-lg">
            下载 macOS 版
          </Link>
          <Link to="/pricing" className="btn btn-secondary btn-lg">
            查看定价
          </Link>
        </div>
        <div className="shot">
          <div className="shot-body" style={{ height: 300 }}>
            {TERM_SHOT.map((l, i) => (
              <div key={i}>{l || "\u00A0"}</div>
            ))}
          </div>
        </div>
      </section>

      <section className="cols3" id="features" style={{ gridTemplateColumns: "repeat(2, 1fr)", maxWidth: 820 }}>
        <div className="card">
          <div className="icon">🛡️</div>
          <h3>防封方案</h3>
          <p>
            独立服务器环境＋固定 IP＋持久 tmux 会话，与本机环境完全隔离，
            大幅降低封号风险。即使连接中断，对话上下文也不会丢失。
          </p>
        </div>
        <div className="card">
          <div className="icon">🔄</div>
          <h3>双向安全同步</h3>
          <p>
            远端改动实时回到本机，本机编辑实时上到服务器。机密文件（.env、密钥）
            默认永不同步；出现冲突时并排对比，绝不静默覆盖。
          </p>
        </div>
      </section>

      <div
        className="banner ok"
        style={{
          maxWidth: 700, margin: "56px auto 0", justifyContent: "center",
          border: invited ? "2px solid var(--green)" : undefined,
        }}
      >
        🎁 获朋友邀请？<Link to="/i/demo">经邀请链接</Link>注册并登录 APP，即享首月免费试用（每个账号限一次）。
      </div>

      <h2 className="section-title">简单定价</h2>
      <p className="section-sub">一个价钱，功能全开，没有其他限制。</p>
      <PricingCards compact />
    </>
  );
}

/* ---------- 邀请落地页 /i/{code} ---------- */

export function InviteLanding() {
  const { code } = useParams();
  const { setInvited } = useDemo();
  const nav = useNavigate();
  return (
    <div className="auth-page">
      <div className="auth-card" style={{ width: 440 }}>
        <div style={{ fontSize: 40, marginBottom: 8 }}>🎁</div>
        <h2>Alex 邀请你使用CC避风港</h2>
        <p className="sub" style={{ marginTop: 10 }}>
          在自己的服务器上安心运行 Claude Code，不再担心封号。
          <br />
          <strong>注册并登录 APP，即享首月免费试用。</strong>
        </p>
        <button
          className="btn btn-primary btn-lg"
          style={{ width: "100%", marginBottom: 10 }}
          onClick={() => {
            setInvited(true);
            nav("/signup");
          }}
        >
          注册领取首月免费
        </button>
        <Link to="/download" className="btn btn-secondary" style={{ display: "block" }} onClick={() => setInvited(true)}>
          先下载 macOS 版
        </Link>
        <div className="terms">
          邀请码 {code} 已自动记录，无需手动输入。每个账号只可享用一次免费试用。
        </div>
      </div>
    </div>
  );
}

/* ---------- 定价 ---------- */

const FAQS = [
  ["有没有免费版？", "没有免费版，但经朋友的邀请链接注册并登录 APP，即可免费试用一个月，功能与正式订阅完全一样。"],
  ["首月免费试用怎么领取？", "经朋友的邀请链接下载、注册并首次登录 APP 后自动发放。每个账号一生只可享用一次。"],
  ["订阅有什么限制？", "没有。不限工作区数量、不限同步用量，所有功能全开，就一个价钱。"],
  ["你们会存储我的源代码吗？", "文件内容只在你的 Mac 与你的服务器之间直接同步；控制面只存储账号与工作区元数据。"],
  ["可以随时取消吗？", "可以。订阅会维持到本期结束，之后不再扣款；你的文件不会被删除。"],
];

export function Pricing() {
  const { showErrors } = useDemo();
  const [open, setOpen] = useState<number | null>(null);
  const [retryKey, setRetryKey] = useState(0);

  if (showErrors && retryKey === 0) {
    return (
      <div style={{ maxWidth: 640, margin: "100px auto", padding: "0 24px" }}>
        <div className="banner error">
          <span>无法加载定价信息，请检查网络后重试。</span>
          <button className="btn btn-secondary" onClick={() => setRetryKey(1)}>
            重试
          </button>
        </div>
      </div>
    );
  }

  return (
    <>
      <h2 className="section-title" style={{ marginTop: 70 }}>
        定价
      </h2>
      <p className="section-sub">价格由后台套餐目录提供，页面不写死数值。</p>
      <PricingCards />
      <div className="faq">
        {FAQS.map(([q, a], i) => (
          <div className="faq-item" key={i}>
            <button onClick={() => setOpen(open === i ? null : i)}>
              {q} <span>{open === i ? "−" : "+"}</span>
            </button>
            {open === i && <div className="a">{a}</div>}
          </div>
        ))}
      </div>
    </>
  );
}

function PricingCards({ compact }: { compact?: boolean }) {
  return (
    <div className="pricing-grid" style={{ gridTemplateColumns: "380px", ...(compact ? { marginTop: 30 } : {}) }}>
      <div className="plan featured">
        <span className="tag">唯一套餐</span>
        <h3>CC避风港包月</h3>
        <div className="price">¥68</div>
        <div className="per">每月 · 随时取消</div>
        <ul>
          <li>防封服务器环境</li>
          <li>双向安全同步</li>
          <li>持久终端（tmux）</li>
          <li>不限工作区数量</li>
          <li>邮件支持</li>
        </ul>
        <Link to="/signup" className="btn btn-primary" style={{ display: "block", textAlign: "center" }}>
          立即订阅
        </Link>
        <p style={{ fontSize: 12, color: "var(--green)", marginTop: 10, lineHeight: 1.5 }}>
          🎁 经朋友邀请注册并登录 APP，首月免费（每个账号限一次）。
        </p>
      </div>
    </div>
  );
}

/* ---------- 下载 ---------- */

export function Download() {
  const { toast } = useDemo();
  return (
    <div className="dl-hero">
      <h1 style={{ fontSize: 36 }}>下载CC避风港 APP</h1>
      <p className="section-sub" style={{ marginTop: 12 }}>
        版本 1.4.2 · 2026年8月8日更新 · 需要 macOS 13 及以上
      </p>
      <div className="ctas" style={{ display: "flex", gap: 14, justifyContent: "center" }}>
        <button className="btn btn-primary btn-lg" onClick={() => toast("正在下载 CCHaven-1.4.2-arm64.dmg…")}>
          下载 macOS 版（Apple Silicon）
        </button>
      </div>
      <p style={{ marginTop: 12, fontSize: 13.5 }}>
        <a href="#" onClick={(e) => { e.preventDefault(); toast("正在下载 CCHaven-1.4.2-x64.dmg…"); }}>
          下载 Intel 版
        </a>
      </p>
      <div className="steps3">
        <div className="step">
          <div className="num">1</div>
          打开 DMG，将「CC避风港」拖入「应用程序」文件夹。
        </div>
        <div className="step">
          <div className="num">2</div>
          启动应用，点「通过浏览器登录」完成授权。
        </div>
        <div className="step">
          <div className="num">3</div>
          连接你的服务器，创建第一个工作区。
        </div>
      </div>
      <p style={{ marginTop: 40, color: "var(--gray)", fontSize: 14 }}>
        已安装？<Link to="/app">打开CC避风港 APP</Link>
      </p>
    </div>
  );
}
