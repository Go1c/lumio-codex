import type { ReactNode } from "react";
import { Link, useParams } from "react-router-dom";

import { GUIDES_EN } from "@/guides.en";
import { GUIDES, type Guide } from "@/guides";

export type GuideLocale = "zh" | "en";

/** 指南正文允许 **粗体**、`代码` 与 [文字](链接) 三种行内标记，不引入 Markdown 依赖。 */
const INLINE_PATTERN = /\*\*([^*]+)\*\*|`([^`]+)`|\[([^\]]+)\]\(([^)]+)\)/g;

const COPY = {
  zh: {
    crumb: "指南",
    indexTitle: "常见问题的完整回答",
    indexLede: "比帮助中心更长的说明：为什么会这样、有哪些做法、BestCodex 在其中做什么。",
    listLabel: "指南列表",
    othersLabel: "其他指南",
    missingTitle: "没有这篇指南",
    missingLede: "链接可能已过期。回到指南列表另选一篇。",
    back: "回到指南列表",
    updated: "最后更新",
    otherLanguage: "English",
  },
  en: {
    crumb: "Guides",
    indexTitle: "Common questions, answered in full",
    indexLede:
      "Longer than the help centre: why it happens, what the options are, and what BestCodex does about it.",
    listLabel: "Guide list",
    othersLabel: "Other guides",
    missingTitle: "No such guide",
    missingLede: "The link may be out of date. Head back and pick another one.",
    back: "Back to guides",
    updated: "Last updated",
    otherLanguage: "中文",
  },
} satisfies Record<GuideLocale, Record<string, string>>;

function guidesFor(locale: GuideLocale): Guide[] {
  return locale === "en" ? GUIDES_EN : GUIDES;
}

function basePath(locale: GuideLocale): string {
  return locale === "en" ? "/en/guides" : "/guides";
}

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

export function GuideIndex({ locale = "zh" }: { locale?: GuideLocale } = {}) {
  const copy = COPY[locale];
  const base = basePath(locale);

  return (
    <div className="help-page">
      <p className="help-crumbs">{copy.crumb}</p>
      <h1>{copy.indexTitle}</h1>
      <p className="help-lede">{copy.indexLede}</p>
      <nav className="help-grid" aria-label={copy.listLabel}>
        {guidesFor(locale).map((guide) => (
          <Link key={guide.slug} className="help-card" to={`${base}/${guide.slug}`}>
            <h2>{guide.question}</h2>
            <p>{guide.summary}</p>
          </Link>
        ))}
      </nav>
      {/* 索引页的语言切换与页脚那条落点相同，只留页脚一条，不给重复链接。 */}
    </div>
  );
}

export function GuideArticle({ locale = "zh" }: { locale?: GuideLocale } = {}) {
  const { slug } = useParams<{ slug: string }>();
  const copy = COPY[locale];
  const base = basePath(locale);
  const guides = guidesFor(locale);
  const guide = guides.find((item) => item.slug === slug);

  if (!guide) {
    return (
      <div className="help-page">
        <p className="help-crumbs">
          <Link to={base}>{copy.crumb}</Link>
        </p>
        <h1>{copy.missingTitle}</h1>
        <p className="help-lede">{copy.missingLede}</p>
        <p>
          <Link to={base} className="btn btn-primary">
            {copy.back}
          </Link>
        </p>
      </div>
    );
  }

  const others = guides.filter((item) => item.slug !== guide.slug);
  // slug 两种语言一致，所以语言切换总能落到对应的那一篇。
  const otherLanguageHref =
    locale === "en" ? `/guides/${guide.slug}` : `/en/guides/${guide.slug}`;

  return (
    <article className="help-page">
      <p className="help-crumbs">
        <Link to={base}>{copy.crumb}</Link>
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

      <nav className="help-grid" aria-label={copy.othersLabel}>
        {others.map((item) => (
          <Link key={item.slug} className="help-card" to={`${base}/${item.slug}`}>
            <h2>{item.question}</h2>
            <p>{item.summary}</p>
          </Link>
        ))}
      </nav>

      <p className="help-canonical">
        {copy.updated} <span>{guide.updated}</span>
        {" · "}
        <Link to={otherLanguageHref}>{copy.otherLanguage}</Link>
      </p>
    </article>
  );
}
