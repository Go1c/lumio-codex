import { Link } from "react-router-dom";

import { PlanCard } from "@/components/PlanCard";
import { TERMINAL_SHOT, VALUES } from "@/content";

export function Home() {
  return (
    <>
      <section className="hero">
        <h1>
          安心使用 Claude Code
          <br />
          不再担心封号
        </h1>
        <p className="sub">
          Claude Code 运行在你自己的服务器上——独立环境、固定 IP、持久会话；文件双向安全同步，编辑体验如同本机。
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
          <div className="shot-body" aria-label="CC避风港 终端界面截图">
            {TERMINAL_SHOT.map((line, index) => (
              <div key={index}>{line || "\u00A0"}</div>
            ))}
          </div>
        </div>
      </section>

      <section className="value-cols" aria-label="核心价值">
        {VALUES.map((value) => (
          <div className="card" key={value.title}>
            <div className="icon" aria-hidden="true">
              {value.icon}
            </div>
            <h3>{value.title}</h3>
            <p>{value.body}</p>
          </div>
        ))}
      </section>

      <div className="banner ok invite-strip">
        <span>🎁 获朋友邀请？注册并登录 APP，即享首月免费试用（每个账号限一次）。</span>
      </div>

      <h2 className="section-title">简单定价</h2>
      <p className="section-sub">一个价钱，功能全开，没有其他限制。</p>
      <PlanCard />

      <div className="ctas bottom-cta">
        <Link to="/download" className="btn btn-primary btn-lg">
          下载并开始试用
        </Link>
      </div>
    </>
  );
}
