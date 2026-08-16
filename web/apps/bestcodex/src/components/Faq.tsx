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
