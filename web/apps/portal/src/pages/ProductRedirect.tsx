import { useEffect } from "react";

import { sessionTokensForHandoff, withHandoff } from "@lumio/auth";
import { cookieDomainFor, productSiteOrigin } from "@lumio/ui";

import { goExternal } from "@/lib/redirect";

export function ProductRedirect({
  path,
  label,
}: {
  path: "/codex" | "/claude";
  label: string;
}) {
  const href = outboundProductUrl(`${productSiteOrigin()}${path}`);

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

function outboundProductUrl(url: string): string {
  const tokens = sessionTokensForHandoff();
  if (!tokens) return url;
  let dest: URL;
  try {
    dest = new URL(url);
  } catch {
    return url;
  }
  if (typeof window !== "undefined") {
    const here = window.location.hostname;
    if (here === dest.hostname) return url;
    const hereJar = cookieDomainFor(here);
    const destJar = cookieDomainFor(dest.hostname);
    if (hereJar && destJar && hereJar === destJar) return url;
  }
  return withHandoff(url, tokens);
}
