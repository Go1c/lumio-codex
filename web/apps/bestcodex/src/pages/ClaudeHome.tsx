import { ProductDownloads, Reveal } from "@lumio/ui";

import { Faq } from "@/components/Faq";
import { PlanCard } from "@/components/PlanCard";
import { CLAUDE_FAQS, TERMINAL_SHOT, VALUES } from "@/content";
import { CLAUDE_FAQS_EN, CLAUDE_HERO_EN, TERMINAL_SHOT_EN, VALUES_EN } from "@/content.en";

function TerminalShot({ lines, label }: { lines: string[]; label: string }) {
  const lastIndex = lines.length - 1;
  return (
    <div className="term-window" role="img" aria-label={label}>
      <div className="term-bar" aria-hidden="true">
        <span className="term-dot r" />
        <span className="term-dot y" />
        <span className="term-dot g" />
        <span className="term-title">claude · my-project</span>
      </div>
      <div className="term-body" aria-hidden="true">
        {lines.map((line, index) => (
          <div
            key={index}
            className="term-line"
            style={{ animationDelay: `${0.4 + index * 0.22}s` }}
          >
            {index === lastIndex ? (
              <>
                {line.replace(/_$/, "")}
                <span className="caret" />
              </>
            ) : (
              line || "\u00A0"
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

export function ClaudeHome({ locale = "zh" }: { locale?: "zh" | "en" } = {}) {
  const en = locale === "en";
  const hero = CLAUDE_HERO_EN;
  const values = en ? VALUES_EN : VALUES;

  return (
    <>
      <div className="hero-split">
        <section className="hero is-claude">
          <Reveal immediate>
            <p className="hero-kicker">{en ? hero.kicker : "为了防封"}</p>
          </Reveal>
          <Reveal immediate delay={0.08}>
            <h1>
              {en ? hero.titleLead : "安心使用 Claude Code"}
              <br />
              <em>{en ? hero.titleEm : "不再担心封号"}</em>
            </h1>
          </Reveal>
          <Reveal immediate delay={0.16}>
            <p className="sub">
              {en
                ? hero.sub
                : "官方 Claude Code 跑在你自己的服务器上——独立环境、固定 IP、持久会话。文件双向同步，编辑体验如同本机。"}
            </p>
          </Reveal>
          <Reveal immediate delay={0.24}>
            <div className="ctas">
              <a className="btn btn-primary btn-lg" href="#downloads">
                {en ? hero.download : "下载 BestCodex"}
              </a>
              <a className="btn btn-secondary btn-lg" href="#pricing">
                {en ? hero.pricing : "查看定价"}
              </a>
            </div>
          </Reveal>
        </section>
        <Reveal immediate delay={0.2} y={36}>
          <TerminalShot
            lines={en ? TERMINAL_SHOT_EN : TERMINAL_SHOT}
            label={en ? hero.termLabel : "Claude 终端界面"}
          />
        </Reveal>
      </div>

      <section className="mk-section" aria-labelledby="why-title">
        <Reveal>
          <span className="section-kicker">Why</span>
          <h2 className="section-title" id="why-title">
            {en ? hero.whyTitle : "防封，以及同步"}
          </h2>
        </Reveal>
        <div className="value-cols">
          {values.map((value, index) => (
            <Reveal className="card" key={value.title} delay={index * 0.1}>
              <h3>{value.title}</h3>
              <p>{value.body}</p>
            </Reveal>
          ))}
        </div>
      </section>

      <section className="mk-section" id="pricing" aria-labelledby="price-title">
        <Reveal>
          <span className="section-kicker">Pricing</span>
          <h2 className="section-title" id="price-title">
            {en ? hero.priceTitle : "简单定价"}
          </h2>
        </Reveal>
        <Reveal delay={0.08}>
          <PlanCard locale={locale} />
        </Reveal>
      </section>

      <ProductDownloads locale={locale} />

      <Faq items={en ? CLAUDE_FAQS_EN : CLAUDE_FAQS} />
    </>
  );
}
