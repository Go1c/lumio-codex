import type { ReactNode } from "react";
import { Link } from "react-router-dom";

import { Aurora, ClaudeMark, OpenAIMark, Reveal, ScrollHint, siteUrl } from "@lumio/ui";

interface Product {
  id: "codex" | "cc";
  kicker: string;
  name: string;
  oneLiner: string;
  points: string[];
  cta: string;
  mark: ReactNode;
  panelClass: string;
}

/** 左 Codex、右 Claude：顺序即布局，图标即身份。 */
const PRODUCTS: Product[] = [
  {
    id: "codex",
    kicker: "OpenAI Codex",
    name: "Lumio Codex",
    oneLiner: "官方 Codex 的快速接入工具——注册、登录与本机配置一步到位，你用的始终是官方应用。",
    points: ["免手动配置，几分钟接入官方 Codex", "macOS / Windows 桌面客户端", "余额与充值走同一个 Lumio 账号"],
    cta: "进入 Lumio Codex",
    mark: <OpenAIMark size={48} />,
    panelClass: "panel-codex",
  },
  {
    id: "cc",
    kicker: "Claude Code",
    name: "CC避风港",
    oneLiner: "Claude Code 的云端避风港——独立环境、固定 IP、持久会话，文件双向安全同步。",
    points: ["独立服务器环境，大幅降低封号风险", "本机与云端双向实时同步", "连接中断也不丢对话上下文"],
    cta: "进入 CC避风港",
    mark: <ClaudeMark size={48} />,
    panelClass: "panel-claude",
  },
];

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
          <p className="sub">一个账号，两件趁手的 AI 开发利器——注册、登录、余额与充值统一在 Lumio 官网完成。</p>
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
          <ScrollHint label="向下滚动 · 看两个产品" />
        </Reveal>
      </section>

      <div className="product-split">
        {PRODUCTS.map((product, index) => (
          <Reveal key={product.id} delay={index * 0.12}>
            <a className={`product-panel ${product.panelClass}`} href={siteUrl(product.id)}>
              <span className="panel-mark" aria-hidden="true">
                {product.mark}
              </span>
              <span className="panel-kicker">{product.kicker}</span>
              <h3>{product.name}</h3>
              <p className="panel-one-liner">{product.oneLiner}</p>
              <ul className="panel-points">
                {product.points.map((point) => (
                  <li key={point}>{point}</li>
                ))}
              </ul>
              <span className="panel-cta">
                {product.cta}
                <span className="arrow" aria-hidden="true">
                  →
                </span>
              </span>
            </a>
          </Reveal>
        ))}
      </div>

      <Reveal>
        <section className="cta-band">
          <h2>开启你的 AI 开发新体验</h2>
          <p>一个账号，解锁全部能力。现在加入 Lumio。</p>
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
