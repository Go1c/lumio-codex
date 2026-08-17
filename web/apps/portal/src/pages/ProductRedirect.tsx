import { useEffect } from "react";

import { productSiteOrigin } from "@lumio/ui";

import { goExternal } from "@/lib/redirect";

export function ProductRedirect({
  path,
  label,
}: {
  path: "/codex" | "/claude";
  label: string;
}) {
  const href = `${productSiteOrigin()}${path}`;

  useEffect(() => {
    goExternal(href);
  }, [href]);

  return (
    <div className="dl-hero">
      <h1 style={{ fontSize: 32 }}>正在前往产品站</h1>
      <p className="section-sub">如果没有自动跳转，请点下面的链接。</p>
      <a href={href} className="btn btn-primary">
        前往 {label}
      </a>
    </div>
  );
}
