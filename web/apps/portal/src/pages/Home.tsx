import { Link } from "react-router-dom";

import { siteUrl } from "@lumio/ui";

const PRODUCTS = [
  {
    id: "codex" as const,
    name: "Lumio Codex",
    nameEn: "官方 Codex 快速接入工具",
    body: "帮你完成注册、登录与本机配置，省去手动安装配置的步骤。你使用的始终是官方 Codex 应用。",
    cta: "了解 Lumio Codex",
  },
  {
    id: "cc" as const,
    name: "CC避风港",
    nameEn: "CCHaven",
    body: "Claude Code 运行在你自己的服务器上——独立环境、固定 IP、持久会话；文件双向安全同步，编辑体验如同本机。",
    cta: "了解 CC避风港",
  },
];

export function Home() {
  return (
    <>
      <section className="hero">
        <h1>
          一个 Lumio 账号
          <br />
          两款趁手的开发工具
        </h1>
        <p className="sub">
          注册、登录、余额与充值统一在 Lumio 官网完成；产品站只管产品本身。
        </p>
        <div className="ctas">
          <Link to="/signup" className="btn btn-primary btn-lg">
            创建账号
          </Link>
          <Link to="/login" className="btn btn-secondary btn-lg">
            登录
          </Link>
        </div>
      </section>

      <h2 className="section-title">两个产品</h2>
      <p className="section-sub">各自独立使用，共用同一个账号与余额。</p>

      <div className="product-grid">
        {PRODUCTS.map((product) => (
          <article className="product-card" key={product.id}>
            <h3>
              {product.name} <span className="product-en">{product.nameEn}</span>
            </h3>
            <p>{product.body}</p>
            <a className="btn btn-primary" href={siteUrl(product.id)}>
              {product.cta}
            </a>
          </article>
        ))}
      </div>

      <div className="ctas bottom-cta">
        <Link to="/signup" className="btn btn-primary btn-lg">
          创建账号
        </Link>
      </div>
    </>
  );
}
