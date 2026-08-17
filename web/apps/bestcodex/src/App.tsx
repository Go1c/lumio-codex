import { useEffect, useLayoutEffect } from "react";
import { Navigate, Route, Routes, useLocation } from "react-router-dom";

import { useSession } from "@lumio/auth";
import { HelpArticle, HelpIndex, SiteShell, helpTopicBySlug, portalAccountLinks } from "@lumio/ui";

import { ClaudeHome } from "@/pages/ClaudeHome";
import { CodexHome } from "@/pages/CodexHome";
import { NotFound } from "@/pages/NotFound";

export function productPageTitle(pathname: string): string {
  if (pathname === "/" || pathname === "/codex" || pathname === "/download") {
    return "Codex · BestCodex";
  }
  if (pathname === "/claude" || pathname === "/pricing") {
    return "Claude · BestCodex";
  }
  if (pathname === "/help") return "帮助 · BestCodex";
  const helpMatch = /^\/help\/([^/]+)$/.exec(pathname);
  if (helpMatch) {
    const topic = helpTopicBySlug(helpMatch[1]);
    return topic ? `${topic.title} · BestCodex` : "没有这篇说明 · BestCodex";
  }
  return "页面不存在 · BestCodex";
}

function RouteTitle() {
  const { pathname } = useLocation();
  useEffect(() => {
    document.title = productPageTitle(pathname);
  }, [pathname]);
  return null;
}

function HashScroll() {
  const { hash, pathname } = useLocation();
  useLayoutEffect(() => {
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
  const currentUrl = `${window.location.origin}${location.pathname}${location.search}`;
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
        <Route path="/pricing" element={<Navigate to="/claude#pricing" replace />} />
        <Route path="/download" element={<Navigate to="/#downloads" replace />} />
        <Route path="/help" element={<HelpIndex />} />
        <Route path="/help/:slug" element={<HelpArticle />} />
        <Route path="*" element={<NotFound />} />
      </Routes>
    </SiteShell>
  );
}
