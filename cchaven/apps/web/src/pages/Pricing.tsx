import { useId, useState } from "react";

import { PlanCard } from "@/components/PlanCard";
import { useT } from "@/i18n";

const FAQS: Array<[string, string]> = [
  [
    "有没有免费版？",
    "没有免费版，但经朋友的邀请链接注册并登录 APP，即可免费试用一个月，功能与正式订阅完全一样。",
  ],
  [
    "首月免费试用怎么领取？",
    "经朋友的邀请链接下载、注册并首次登录 APP 后自动发放。每个账号一生只可享用一次。",
  ],
  ["订阅有什么限制？", "没有。不限工作区数量、不限同步用量，所有功能全开，就一个价钱。"],
  [
    "你们会存储我的源代码吗？",
    "文件内容只在你的 Mac 与你的服务器之间直接同步；控制面只存储账号与工作区元数据。",
  ],
  ["可以随时取消吗？", "可以。订阅会维持到本期结束，之后不再扣款；你的文件不会被删除。"],
];

/** 4.2 定价页：单一套餐卡片 + FAQ 手风琴（同时只展开一项）。 */
export function Pricing() {
  const t = useT();
  const [open, setOpen] = useState<number | null>(null);
  const baseId = useId();

  return (
    <>
      <h2 className="section-title" style={{ marginTop: 70 }}>
        {t("pricing.title")}
      </h2>
      <p className="section-sub">{t("pricing.subtitle")}</p>

      <PlanCard />

      <section className="faq" aria-label={t("pricing.faq_title")}>
        <h3 className="sr-only">{t("pricing.faq_title")}</h3>
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
    </>
  );
}
