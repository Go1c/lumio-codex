import { useId, useState } from "react";

import { Reveal } from "@lumio/ui";

export function Faq({ items, defaultOpen = null }: { items: Array<[string, string]>; defaultOpen?: number | null }) {
  const [open, setOpen] = useState<number | null>(defaultOpen);
  const baseId = useId();

  return (
    <section className="faq mk-section" id="faq" aria-label="常见问题">
      <Reveal>
        <span className="section-kicker">FAQ</span>
        <h2 className="section-title">常见问题</h2>
      </Reveal>
      {items.map(([question, answer], index) => {
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
            {/*
              答案始终渲染，折叠时只用 hidden 收起。搜索引擎与 AI 引擎都靠首屏 HTML
              抽取问答对；条件渲染会让折叠项的答案根本不进 HTML。
            */}
            <div className="a" id={panelId} hidden={!expanded}>
              {answer}
            </div>
          </div>
        );
      })}
    </section>
  );
}
