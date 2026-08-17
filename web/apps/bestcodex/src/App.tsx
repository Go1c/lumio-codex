import { useEffect, useLayoutEffect } from "react";
import { Navigate, Route, Routes, useLocation } from "react-router-dom";

import { useSession } from "@lumio/auth";
import {
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
    if (!hash) return;
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

  return (
    <SiteShell
      site={onClaude ? "cc" : "codex"}
      brand={{ name: "BestCodex" }}
      account={{ status: session.status, email: session.profile?.email }}
      accountLinks={portalAccountLinks(currentUrl)}
      downloadHref={onClaude ? "/claude#downloads" : "/#downloads"}
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
        <Route path="*" element={<NotFound />} />
      </Routes>
    </SiteShell>
  );
}
