import { ProductDownloads, Reveal } from "@lumio/ui";

import { Faq } from "@/components/Faq";
import { PlanCard } from "@/components/PlanCard";
import { CLAUDE_FAQS, TERMINAL_SHOT, VALUES } from "@/content";

function TerminalShot() {
  const lastIndex = TERMINAL_SHOT.length - 1;
  return (
    <div className="term-window" role="img" aria-label="Claude 终端界面">
      <div className="term-bar" aria-hidden="true">
        <span className="term-dot r" />
        <span className="term-dot y" />
        <span className="term-dot g" />
        <span className="term-title">claude · my-project</span>
      </div>
      <div className="term-body" aria-hidden="true">
        {TERMINAL_SHOT.map((line, index) => (
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

export function ClaudeHome() {
  return (
    <>
      <div className="hero-split">
        <section className="hero is-claude">
          <Reveal immediate>
            <p className="hero-kicker">为了防封</p>
          </Reveal>
          <Reveal immediate delay={0.08}>
            <h1>
              安心使用 Claude Code
              <br />
              <em>不再担心封号</em>
            </h1>
          </Reveal>
          <Reveal immediate delay={0.16}>
            <p className="sub">
              官方 Claude Code 跑在你自己的服务器上——独立环境、固定 IP、持久会话。文件双向同步，编辑体验如同本机。
            </p>
          </Reveal>
          <Reveal immediate delay={0.24}>
            <div className="ctas">
              <a className="btn btn-primary btn-lg" href="#downloads">
                下载 BestCodex
              </a>
              <a className="btn btn-secondary btn-lg" href="#pricing">
                查看定价
              </a>
            </div>
          </Reveal>
        </section>
        <Reveal immediate delay={0.2} y={36}>
          <TerminalShot />
        </Reveal>
      </div>

      <section className="mk-section" aria-labelledby="why-title">
        <Reveal>
          <span className="section-kicker">Why</span>
          <h2 className="section-title" id="why-title">
            防封，以及同步
          </h2>
        </Reveal>
        <div className="value-cols">
          {VALUES.map((value, index) => (
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
            简单定价
          </h2>
        </Reveal>
        <Reveal delay={0.08}>
          <PlanCard />
        </Reveal>
      </section>

      <ProductDownloads />

      <Faq items={CLAUDE_FAQS} />
    </>
  );
}
