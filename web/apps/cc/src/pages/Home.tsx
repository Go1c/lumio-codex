import { Link } from "react-router-dom";

import { Aurora, Reveal, ScrollHint } from "@lumio/ui";

import { PlanCard } from "@/components/PlanCard";
import { TERMINAL_SHOT, VALUES } from "@/content";

/** 玻璃终端窗口：逐行浮现 + 光标闪烁，还原「云端会话」的现场感。 */
function TerminalShot() {
  const lastIndex = TERMINAL_SHOT.length - 1;
  return (
    <div className="term-window" role="img" aria-label="CC避风港 终端界面截图">
      <div className="term-bar" aria-hidden="true">
        <span className="term-dot r" />
        <span className="term-dot y" />
        <span className="term-dot g" />
        <span className="term-title">cc-haven — claude · tmux</span>
      </div>
      <div className="term-body" aria-hidden="true">
        {TERMINAL_SHOT.map((line, index) => (
          <div
            key={index}
            className="term-line"
            style={{ animationDelay: `${0.4 + index * 0.22}s` }}
          >
            {index === lastIndex ? (
              <>
                {line.replace(/_$/, "")}
                <span className="caret" />
              </>
            ) : (
              line || "\u00A0"
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

export function Home() {
  return (
    <>
      <Aurora variant="claude" />

      <section className="hero-split">
        <div>
          <Reveal immediate>
            <span className="hero-badge">为 Claude Code 而生</span>
          </Reveal>
          <Reveal immediate delay={0.08}>
            <h1>
              安心使用 Claude Code
              <br />
              <span className="grad-text">不再担心封号</span>
            </h1>
          </Reveal>
          <Reveal immediate delay={0.16}>
            <p className="sub">
              Claude Code 运行在你自己的服务器上——独立环境、固定 IP、持久会话；文件双向安全同步，编辑体验如同本机。
            </p>
          </Reveal>
          <Reveal immediate delay={0.24}>
            <div className="ctas">
              <Link to="/download" className="btn btn-primary btn-lg">
                下载 macOS 版
              </Link>
              <Link to="/pricing" className="btn btn-secondary btn-lg">
                查看定价
              </Link>
            </div>
          </Reveal>
        </div>
        <Reveal immediate delay={0.2} y={36}>
          <TerminalShot />
        </Reveal>
      </section>

      <Reveal immediate delay={0.4}>
        <ScrollHint label="向下滚动 · 了解防封与同步" />
      </Reveal>

      <section className="value-cols" aria-label="核心价值">
        {VALUES.map((value, index) => (
          <Reveal className="card" key={value.title} delay={index * 0.1}>
            <div className="icon" aria-hidden="true">
              {value.icon}
            </div>
            <h3>{value.title}</h3>
            <p>{value.body}</p>
          </Reveal>
        ))}
      </section>

      <Reveal>
        <div className="banner ok invite-strip">
          <span>🎁 获朋友邀请？注册并登录 APP，即享首月免费试用（每个账号限一次）。</span>
        </div>
      </Reveal>

      <Reveal>
        <span className="section-kicker">Pricing</span>
        <h2 className="section-title">简单定价</h2>
        <p className="section-sub">一个价钱，功能全开，没有其他限制。</p>
      </Reveal>
      <Reveal delay={0.08}>
        <PlanCard />
      </Reveal>

      <Reveal>
        <section className="cta-band">
          <h2>把 Claude Code 搬进避风港</h2>
          <p>下载 macOS 客户端，几分钟完成连接，随时随地接管你的会话。</p>
          <div className="ctas">
            <Link to="/download" className="btn btn-primary btn-lg">
              下载并开始试用
            </Link>
          </div>
        </section>
      </Reveal>
    </>
  );
}
