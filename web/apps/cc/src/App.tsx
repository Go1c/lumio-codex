import { Link, Route, Routes, useLocation } from "react-router-dom";

import { useSession } from "@lumio/auth";
import { HelpArticle, HelpIndex, SiteShell, portalAccountLinks } from "@lumio/ui";

import { Download } from "@/pages/Download";
import { Home } from "@/pages/Home";
import { Pricing } from "@/pages/Pricing";

export function App() {
  const session = useSession();
  const location = useLocation();
  const currentUrl = `${window.location.origin}${location.pathname}${location.search}`;

  return (
    <SiteShell
      site="cc"
      brand={{ name: "BestCodex" }}
      account={{ status: session.status, email: session.profile?.email }}
      accountLinks={portalAccountLinks(currentUrl)}
    >
      <Routes>
        <Route path="/" element={<Home />} />
        <Route path="/pricing" element={<Pricing />} />
        <Route path="/download" element={<Download />} />
        <Route path="/help" element={<HelpIndex />} />
        <Route path="/help/:slug" element={<HelpArticle />} />
        <Route path="*" element={<NotFound />} />
      </Routes>
    </SiteShell>
  );
}

function NotFound() {
  return (
    <div className="dl-hero">
      <h1 style={{ fontSize: 32 }}>页面不存在</h1>
      <p className="section-sub">链接可能已过期，或地址输错了。</p>
      <Link to="/" className="btn btn-primary">
        回到首页
      </Link>
    </div>
  );
}
