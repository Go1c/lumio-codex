import { Navigate, Route, Routes, useLocation } from "react-router-dom";

import { useSession } from "@lumio/auth";
import { HelpArticle, HelpIndex, SiteShell, portalAccountLinks } from "@lumio/ui";

import { ClaudeHome } from "@/pages/ClaudeHome";
import { CodexHome } from "@/pages/CodexHome";

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
      <Routes>
        <Route path="/" element={<CodexHome />} />
        <Route path="/codex" element={<CodexHome />} />
        <Route path="/claude" element={<ClaudeHome />} />
        <Route path="/pricing" element={<Navigate to="/claude#pricing" replace />} />
        <Route path="/download" element={<Navigate to="/#downloads" replace />} />
        <Route path="/help" element={<HelpIndex />} />
        <Route path="/help/:slug" element={<HelpArticle />} />
      </Routes>
    </SiteShell>
  );
}
