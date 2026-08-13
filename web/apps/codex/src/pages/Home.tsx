import { useId, useState } from "react";

import { Downloads } from "@/components/Downloads";

const STEPS = [
  {
    num: "01",
    title: "下载安装",
    body: "选择你的平台安装。官方 Codex 应用需单独安装，本工具不捆绑、不修改它。",
  },
  {
    num: "02",
    title: "在 App 内登录",
    body: "用 Lumio 账号登录后，连接与本机配置自动完成，无需填写任何服务地址或密钥。",
  },
  {
    num: "03",
    title: "启动官方 Codex",
    body: "点「启动 Codex」进入官方应用开始工作；余额与充值在 Lumio 官网或 App 内完成。",
  },
];

const FAQS: Array<[string, string]> = [
  [
    "Lumio Codex 是官方应用吗？",
    "不是。它是一个独立的辅助接入工具，仅为了让你更快用上官方 Codex、减少安装配置步骤。完成连接后，你日常使用的就是官方 Codex 本身。",
  ],
  [
    "它会修改官方 Codex 吗？",
    "不会。官方应用保持原样；本工具只写入自己管理的连接配置，写入前先备份，随时可一键恢复。",
  ],
  [
    "在哪里注册和充值？",
    "账号统一在 Lumio 官网注册登录，App 内也可直接登录；充值会打开 Sub2API 收银台完成支付。",
  ],
];

export function Home() {
  return (
    <>
      <section className="hero" id="top">
        <h1>更快开始使用官方 Codex</h1>
        <p className="sub">
          Lumio Codex 是一个轻量接入工具：帮你完成注册、登录和本机配置，省去手动安装配置的步骤。你使用的始终是官方 Codex 应用。
        </p>
        <div className="ctas">
          <a className="btn btn-primary btn-lg" href="#downloads">
            下载 Lumio Codex
          </a>
        </div>
      </section>

      <h2 className="section-title">三步开始</h2>
      <div className="steps3">
        {STEPS.map((step) => (
          <div className="step" key={step.num}>
            <div className="num" aria-hidden="true">
              {step.num}
            </div>
            <h3>{step.title}</h3>
            <p>{step.body}</p>
          </div>
        ))}
      </div>

      <Downloads />

      <Faq />
    </>
  );
}

function Faq() {
  const [open, setOpen] = useState<number | null>(0);
  const baseId = useId();

  return (
    <section className="faq" id="faq" aria-label="常见问题">
      <h2 className="section-title">常见问题</h2>
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
