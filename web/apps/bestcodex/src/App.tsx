import { useEffect, useLayoutEffect } from "react";
import { Navigate, Route, Routes, useLocation } from "react-router-dom";

import { isHandoffHash, useSession } from "@lumio/auth";
import {
  EN_SITE_LABELS,
  HelpArticle,
  HelpIndex,
  SiteShell,
  isServerRender,
  portalAccountLinks,
  productSiteOrigin,
} from "@lumio/ui";

import { ClaudeHome } from "@/pages/ClaudeHome";
import { CodexHome } from "@/pages/CodexHome";
import { GuideArticle, GuideIndex } from "@/pages/Guides";
import { NotFound } from "@/pages/NotFound";
import { pageTitle } from "@/seo";

/**
 * 每页可达的站内互链。只进 sitemap 的页面对爬虫是孤岛，指南层必须从落地页链得到。
 * 语言互链放这里而不只放正文里，是为了让两种语言在任何一页都能互相到达。
 */
const ZH_FOOTER_LINKS = [
  { label: "指南", href: "/guides" },
  { label: "帮助中心", href: "/help" },
  { label: "English", href: "/en/guides" },
];

const EN_FOOTER_LINKS = [
  { label: "Guides", href: "/en/guides" },
  { label: "中文", href: "/guides" },
];

function RouteTitle() {
  const { pathname } = useLocation();
  useEffect(() => {
    document.title = pageTitle(pathname);
  }, [pathname]);
  return null;
}

// 预渲染没有布局阶段，useLayoutEffect 会在每条路由上告警。服务端退化成 useEffect：
// 两者在服务端都不执行，只是后者不告警。
const useIsomorphicLayoutEffect = isServerRender() ? useEffect : useLayoutEffect;

function HashScroll() {
  const { hash, pathname } = useLocation();
  useIsomorphicLayoutEffect(() => {
    // 会话交接用的 hash 不是锚点，不能拿去找元素滚动。
    if (!hash || isHandoffHash(hash)) return;
    const id = decodeURIComponent(hash.replace(/^#/, ""));
    if (!id) return;
    document.getElementById(id)?.scrollIntoView?.();
  }, [hash, pathname]);
  return null;
}

export function App() {
  const session = useSession();
  const location = useLocation();
  // 预渲染没有 window；本地联调必须保留真实 origin，否则 `?next=` 会回跳到生产域。
  const origin = isServerRender() ? productSiteOrigin() : window.location.origin;
  const currentUrl = `${origin}${location.pathname}${location.search}`;
  const onClaude = location.pathname.startsWith("/claude");
  // 英文页只有指南层；外壳文案跟着换，「帮助」指回英文指南索引而不是中文帮助中心。
  const onEnglish = location.pathname.startsWith("/en/");

  return (
    <SiteShell
      site={onClaude ? "cc" : "codex"}
      brand={{ name: "BestCodex" }}
      account={{ status: session.status, email: session.profile?.email }}
      accountLinks={portalAccountLinks(currentUrl)}
      downloadHref={onClaude ? "/claude#downloads" : "/#downloads"}
      // 中文站顶栏「指南 / 帮助」两条；英文层只有指南，把那一格换成它，不留死链。
      nav={onEnglish ? [] : [{ label: "指南", href: "/guides" }]}
      footerLinks={onEnglish ? EN_FOOTER_LINKS : ZH_FOOTER_LINKS}
      {...(onEnglish
        ? { labels: { ...EN_SITE_LABELS, help: "Guides" }, helpHref: "/en/guides" }
        : {})}
    >
      <RouteTitle />
      <HashScroll />
      <Routes>
        <Route path="/" element={<CodexHome />} />
        <Route path="/codex" element={<CodexHome />} />
        <Route path="/claude" element={<ClaudeHome />} />
        {/* 生产由部署层 301 处理这两条；SPA 内保留兜底，本地与直接跳转仍可用。 */}
        <Route path="/pricing" element={<Navigate to="/claude#pricing" replace />} />
        <Route path="/download" element={<Navigate to="/#downloads" replace />} />
        <Route path="/help" element={<HelpIndex />} />
        <Route path="/help/:slug" element={<HelpArticle />} />
        <Route path="/guides" element={<GuideIndex />} />
        <Route path="/guides/:slug" element={<GuideArticle />} />
        <Route path="/en/guides" element={<GuideIndex locale="en" />} />
        <Route path="/en/guides/:slug" element={<GuideArticle locale="en" />} />
        <Route path="*" element={<NotFound />} />
      </Routes>
    </SiteShell>
  );
}
