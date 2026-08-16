import { Link, useParams } from "react-router-dom";

import { HELP_TOPICS, helpCanonicalNote, helpTopicBySlug } from "./topics";

export function HelpIndex() {
  const { canonical, product } = helpCanonicalNote();

  return (
    <div className="help-page">
      <p className="help-crumbs">帮助</p>
      <h1>需要什么帮助？</h1>
      <p className="help-lede">
        BestCodex 只负责登录、本机配置和启动官方 Codex。官方应用里的问题，请看 OpenAI 或 Anthropic
        的说明。
      </p>
      <nav className="help-grid" aria-label="帮助主题">
        {HELP_TOPICS.map((topic) => (
          <Link key={topic.slug} className="help-card" to={`/help/${topic.slug}`}>
            <h2>{topic.title}</h2>
            <p>{topic.summary}</p>
          </Link>
        ))}
      </nav>
      <p className="help-canonical">
        规范 URL 是 <span>{canonical}</span>
        。若根域是门户，请打开产品站 <span>{product}</span>。
      </p>
    </div>
  );
}

export function HelpArticle() {
  const { slug } = useParams<{ slug: string }>();
  const topic = helpTopicBySlug(slug);
  const { canonical, product } = helpCanonicalNote();

  if (!topic) {
    return (
      <div className="help-page">
        <p className="help-crumbs">
          <Link to="/help">帮助</Link>
        </p>
        <h1>没有这篇说明</h1>
        <p className="help-lede">链接可能已过期。回到帮助中心另选一篇。</p>
        <p>
          <Link to="/help" className="btn btn-primary">
            回到帮助中心
          </Link>
        </p>
      </div>
    );
  }

  return (
    <article className="help-page">
      <p className="help-crumbs">
        <Link to="/help">帮助</Link>
        {" / "}
        {topic.title}
      </p>
      <h1>{topic.title}</h1>
      {topic.body.map((paragraph) => (
        <p key={paragraph}>{paragraph}</p>
      ))}
      <p className="help-canonical">
        规范 URL 是 <span>{canonical}</span>
        。若根域是门户，请打开产品站 <span>{product}</span>。
      </p>
    </article>
  );
}
