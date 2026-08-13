import { useId, useState } from "react";

import { PlanCard } from "@/components/PlanCard";
import { FAQS } from "@/content";

/** 单一套餐 + FAQ 手风琴（同时只展开一项）。 */
export function Pricing() {
  const [open, setOpen] = useState<number | null>(null);
  const baseId = useId();

  return (
    <>
      <h2 className="section-title" style={{ marginTop: 70 }}>
        定价
      </h2>
      <p className="section-sub">一个价钱，功能全开，没有其他限制。</p>

      <PlanCard />

      <section className="faq" aria-label="常见问题">
        <h3 className="sr-only">常见问题</h3>
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
