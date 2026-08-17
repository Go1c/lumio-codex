import { Route, Routes } from "react-router-dom";

import { useSession } from "@lumio/auth";
import { SiteLink, SiteShell, portalAccountLinks } from "@lumio/ui";

import { Home } from "@/pages/Home";
import { Privacy } from "@/pages/Privacy";
import { Terms } from "@/pages/Terms";

export function App() {
  // 产品站只读会话用于显示账号入口，注册 / 登录 / 充值都在门户。
  const session = useSession();

  return (
    <SiteShell
      site="codex"
      brand={{ name: "Lumio Codex" }}
      nav={[
        { label: "三步开始", href: "/#top" },
        { label: "下载", href: "/#downloads" },
        { label: "常见问题", href: "/#faq" },
      ]}
      account={{ status: session.status, email: session.profile?.email }}
      accountLinks={portalAccountLinks(window.location.href)}
      footerExtra={
        <>
          <SiteLink href="/privacy">隐私政策</SiteLink>
          <SiteLink href="/terms">服务条款</SiteLink>
          <span>
            开源软件（AGPL-3.0-only），fork 自 BigPizzaV3/CodexPlusPlus。OpenAI、Codex、ChatGPT 为其各自所有者的商标，与本项目无从属关系；官方应用需单独安装。
          </span>
        </>
      }
    >
      <Routes>
        <Route path="/" element={<Home />} />
        <Route path="/privacy" element={<Privacy />} />
        <Route path="/terms" element={<Terms />} />
      </Routes>
    </SiteShell>
  );
}
