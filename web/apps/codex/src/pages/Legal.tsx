import type { ReactNode } from "react";
import { useEffect } from "react";
import { Link } from "react-router-dom";

import { Aurora } from "@lumio/ui";

export function LegalPage({ title, children }: { title: string; children: ReactNode }) {
  useEffect(() => {
    const previous = document.title;
    document.title = `${title} · Lumio Codex`;
    return () => {
      document.title = previous;
    };
  }, [title]);

  return (
    <>
      <Aurora variant="codex" />
      <article className="acct">
        <h1 style={{ fontSize: 28, marginBottom: 8 }}>{title}</h1>
        <p className="note" style={{ marginBottom: 22 }}>
          适用于 Lumio Codex（bestcodex.app）。运营主体、地址、联系邮箱与备案/ICP 号将补充。
        </p>
        {children}
        <p className="note" style={{ marginTop: 8 }}>
          <Link to="/">返回首页</Link>
        </p>
      </article>
    </>
  );
}
