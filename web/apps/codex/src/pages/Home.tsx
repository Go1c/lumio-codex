import { useId, useState } from "react";

import { ProductDownloads, Reveal } from "@lumio/ui";

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

const FAQS: Array<[string, string]> = [
  [
    "BestCodex 是官方 Codex 吗？",
    "不是。它是独立的辅助接入工具，仅为了让你更快用上官方 Codex、减少安装配置步骤。完成连接后，你日常使用的就是官方 Codex 本身。",
  ],
  [
    "它会修改官方 Codex 吗？",
    "不会。官方应用保持原样；本工具只写入自己管理的连接配置，写入前先备份，随时可一键恢复。",
  ],
];

export function Home() {
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

      <Faq />
    </>
  );
}

function Faq() {
  const [open, setOpen] = useState<number | null>(0);
  const baseId = useId();

  return (
    <section className="faq mk-section" id="faq" aria-label="常见问题">
      <Reveal>
        <span className="section-kicker">FAQ</span>
        <h2 className="section-title">常见问题</h2>
      </Reveal>
      {FAQS.map(([question, answer], index) => {
        const panelId = `${baseId}-faq-${index}`;
        const expanded = open === index;
        return (
          <div className="faq-item" key={question}>
            <button
              type="button"
              aria-expanded={expanded}
              aria-controls={panelId}
              onClick={() => setOpen(expanded ? null : index)}
            >
              {question}
              <span aria-hidden="true">{expanded ? "−" : "+"}</span>
            </button>
            {expanded && (
              <div className="a" id={panelId}>
                {answer}
              </div>
            )}
          </div>
        );
      })}
    </section>
  );
}
