import { Link } from "react-router-dom";

import { Aurora, Reveal, ScrollHint, siteUrl } from "@lumio/ui";

export function Home() {
  return (
    <>
      <Aurora variant="brand" />

      <section className="hero">
        <Reveal immediate>
          <span className="hero-badge">Lumio · 统一账号中心</span>
        </Reveal>
        <Reveal immediate delay={0.08}>
          <h1>
            官方原生
            <br />
            <span className="grad-text grad-codex">Codex</span> 与{" "}
            <span className="grad-text grad-claude">Claude</span>
          </h1>
        </Reveal>
        <Reveal immediate delay={0.16}>
          <p className="sub">
            一个账号，一个 BestCodex 启动器——注册、登录、余额与充值统一在 Lumio 官网完成。
          </p>
        </Reveal>
        <Reveal immediate delay={0.24}>
          <div className="ctas">
            <Link to="/signup" className="btn btn-primary btn-lg">
              创建账号
            </Link>
            <Link to="/login" className="btn btn-secondary btn-lg">
              登录
            </Link>
          </div>
        </Reveal>
        <Reveal immediate delay={0.4}>
          <ScrollHint label="向下滚动 · 了解 BestCodex" />
        </Reveal>
      </section>

      <div className="product-split">
        <Reveal>
          <a className="product-panel panel-codex" href={siteUrl("codex")}>
            <span className="panel-kicker">一个启动器</span>
            <h3>BestCodex</h3>
            <p className="panel-one-liner">
              下载一次、登录一次。窗口里两个 Tab：Codex 启动官方应用，Claude 把工作跑在你自己的服务器上。
            </p>
            <ul className="panel-points">
              <li>一个启动器，一次下载</li>
              <li>Codex 与 Claude 两种工作方式</li>
              <li>余额与充值走同一个 Lumio 账号</li>
            </ul>
            <span className="panel-cta">
              进入 BestCodex
              <span className="arrow" aria-hidden="true">
                →
              </span>
            </span>
          </a>
        </Reveal>
      </div>

      <Reveal>
        <section className="cta-band">
          <h2>开启你的 AI 开发新体验</h2>
          <p>一个账号，解锁 BestCodex。现在加入 Lumio。</p>
          <div className="ctas">
            <Link to="/signup" className="btn btn-primary btn-lg">
              免费创建账号
            </Link>
          </div>
        </section>
      </Reveal>
    </>
  );
}
