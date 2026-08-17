import type { ReactNode } from "react";
import { Link, useParams } from "react-router-dom";

import { GUIDES, guideBySlug } from "@/guides";

/** 指南正文允许 **粗体**、`代码` 与 [文字](链接) 三种行内标记，不引入 Markdown 依赖。 */
const INLINE_PATTERN = /\*\*([^*]+)\*\*|`([^`]+)`|\[([^\]]+)\]\(([^)]+)\)/g;

function renderInline(text: string, keyPrefix: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  const pattern = new RegExp(INLINE_PATTERN.source, "g");
  let cursor = 0;
  let index = 0;
  let match: RegExpExecArray | null;

  while ((match = pattern.exec(text)) !== null) {
    if (match.index > cursor) nodes.push(text.slice(cursor, match.index));
    const [full, bold, code, linkText, href] = match;
    const key = `${keyPrefix}-${index++}`;

    if (bold) {
      nodes.push(<strong key={key}>{bold}</strong>);
    } else if (code) {
      nodes.push(<code key={key}>{code}</code>);
    } else if (linkText && href) {
      nodes.push(
        href.startsWith("/") ? (
          <Link key={key} to={href}>
            {linkText}
          </Link>
        ) : (
          <a key={key} href={href} target="_blank" rel="noreferrer">
            {linkText}
          </a>
        ),
      );
    }
    cursor = match.index + full.length;
  }
  if (cursor < text.length) nodes.push(text.slice(cursor));
  return nodes;
}

export function GuideIndex() {
  return (
    <div className="help-page">
      <p className="help-crumbs">指南</p>
      <h1>常见问题的完整回答</h1>
      <p className="help-lede">
        比帮助中心更长的说明：为什么会这样、有哪些做法、BestCodex 在其中做什么。
      </p>
      <nav className="help-grid" aria-label="指南列表">
        {GUIDES.map((guide) => (
          <Link key={guide.slug} className="help-card" to={`/guides/${guide.slug}`}>
            <h2>{guide.question}</h2>
            <p>{guide.summary}</p>
          </Link>
        ))}
      </nav>
    </div>
  );
}

export function GuideArticle() {
  const { slug } = useParams<{ slug: string }>();
  const guide = guideBySlug(slug);

  if (!guide) {
    return (
      <div className="help-page">
        <p className="help-crumbs">
          <Link to="/guides">指南</Link>
        </p>
        <h1>没有这篇指南</h1>
        <p className="help-lede">链接可能已过期。回到指南列表另选一篇。</p>
        <p>
          <Link to="/guides" className="btn btn-primary">
            回到指南列表
          </Link>
        </p>
      </div>
    );
  }

  const others = GUIDES.filter((item) => item.slug !== guide.slug);

  return (
    <article className="help-page">
      <p className="help-crumbs">
        <Link to="/guides">指南</Link>
        {" / "}
        {guide.title}
      </p>
      <h1>{guide.title}</h1>
      {/* 答案前置：第一段就是自包含结论，方便搜索引擎与 AI 引擎直接引用。 */}
      <p className="help-lede">{renderInline(guide.answer, "answer")}</p>

      {guide.sections.map((section) => (
        <section key={section.heading}>
          <h2>{section.heading}</h2>
          {section.body.map((paragraph, index) => (
            <p key={paragraph}>{renderInline(paragraph, `${section.heading}-${index}`)}</p>
          ))}
        </section>
      ))}

      <nav className="help-grid" aria-label="其他指南">
        {others.map((item) => (
          <Link key={item.slug} className="help-card" to={`/guides/${item.slug}`}>
            <h2>{item.question}</h2>
            <p>{item.summary}</p>
          </Link>
        ))}
      </nav>

      <p className="help-canonical">
        最后更新 <span>{guide.updated}</span>
      </p>
    </article>
  );
}
