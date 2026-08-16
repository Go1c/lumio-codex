import { ProductDownloads, Reveal } from "@lumio/ui";

import { Faq } from "@/components/Faq";
import { CODEX_FAQS } from "@/content";

const STEPS = [
  {
    num: "01",
    title: "下载安装",
    body: "选择你的平台。官方 Codex 应用需单独安装，本工具不捆绑、不修改它。",
  },
  {
    num: "02",
    title: "在 App 内登录",
    body: "登录后连接与本机配置自动完成，无需填写任何服务地址或密钥。",
  },
  {
    num: "03",
    title: "启动官方 Codex",
    body: "点「启动 Codex」进入官方应用。余额与充值在账户中心完成。",
  },
];

export function CodexHome() {
  return (
    <>
      <section className="hero" id="top">
        <Reveal immediate>
          <p className="hero-kicker">官方原版</p>
        </Reveal>
        <Reveal immediate delay={0.08}>
          <h1>
            更快开始使用
            <br />
            <em>官方 Codex</em>
          </h1>
        </Reveal>
        <Reveal immediate delay={0.16}>
          <p className="sub">
            帮你完成注册、登录和本机配置。你使用的始终是官方 Codex 应用，不捆绑、不修改。
          </p>
        </Reveal>
        <Reveal immediate delay={0.24}>
          <div className="ctas">
            <a className="btn btn-primary btn-lg" href="#downloads">
              下载 BestCodex
            </a>
            <a className="btn btn-secondary btn-lg" href="#faq">
              常见问题
            </a>
          </div>
        </Reveal>
      </section>

      <section className="mk-section" aria-labelledby="start-title">
        <Reveal>
          <span className="section-kicker">Get started</span>
          <h2 className="section-title" id="start-title">
            三步开始
          </h2>
        </Reveal>
        <div className="steps3">
          {STEPS.map((step, index) => (
            <Reveal className="step" key={step.num} delay={index * 0.1}>
              <div className="num" aria-hidden="true">
                {step.num}
              </div>
              <h3>{step.title}</h3>
              <p>{step.body}</p>
            </Reveal>
          ))}
        </div>
      </section>

      <ProductDownloads />

      <Faq items={CODEX_FAQS} defaultOpen={0} />
    </>
  );
}
