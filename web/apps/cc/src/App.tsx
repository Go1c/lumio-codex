import { Link, Route, Routes, useLocation } from "react-router-dom";

import { useSession } from "@lumio/auth";
import { SiteShell, portalAccountLinks } from "@lumio/ui";

import { Download } from "@/pages/Download";
import { Home } from "@/pages/Home";
import { Pricing } from "@/pages/Pricing";

export function App() {
  // 产品站只读会话用于显示账号入口，所有账号操作都在门户完成。
  const session = useSession();
  // 跟随前端路由取当前地址，否则站内跳转后 next 还停在首屏 URL。
  const location = useLocation();
  const currentUrl = `${window.location.origin}${location.pathname}${location.search}`;

  return (
    <SiteShell
      site="cc"
      brand={{ name: "CC避风港", nameEn: "CCHaven" }}
      nav={[
        { label: "定价", href: "/pricing" },
        { label: "下载", href: "/download" },
      ]}
      account={{ status: session.status, email: session.profile?.email }}
      accountLinks={portalAccountLinks(currentUrl)}
    >
      <Routes>
        <Route path="/" element={<Home />} />
        <Route path="/pricing" element={<Pricing />} />
        <Route path="/download" element={<Download />} />
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
