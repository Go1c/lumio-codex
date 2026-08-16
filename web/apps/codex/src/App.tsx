import { Route, Routes, useLocation } from "react-router-dom";

import { useSession } from "@lumio/auth";
import { HelpArticle, HelpIndex, SiteShell, portalAccountLinks } from "@lumio/ui";

import { Home } from "@/pages/Home";

export function App() {
  const session = useSession();
  const location = useLocation();
  const currentUrl = `${window.location.origin}${location.pathname}${location.search}`;

  return (
    <SiteShell
      site="codex"
      brand={{ name: "BestCodex" }}
      account={{ status: session.status, email: session.profile?.email }}
      accountLinks={portalAccountLinks(currentUrl)}
    >
      <Routes>
        <Route path="/" element={<Home />} />
        <Route path="/help" element={<HelpIndex />} />
        <Route path="/help/:slug" element={<HelpArticle />} />
      </Routes>
    </SiteShell>
  );
}
