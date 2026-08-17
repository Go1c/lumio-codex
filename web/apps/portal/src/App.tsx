import { Route, Routes } from "react-router-dom";

import { HelpArticle, HelpIndex, SiteShell, ToastProvider, siteUrl } from "@lumio/ui";

import { Account } from "@/pages/Account";
import { Authorize } from "@/pages/Authorize";
import { Home } from "@/pages/Home";
import { Login } from "@/pages/Login";
import { Logout } from "@/pages/Logout";
import { NotFound } from "@/pages/NotFound";
import { ProductRedirect } from "@/pages/ProductRedirect";
import { Signup } from "@/pages/Signup";
import { SessionProvider, usePortalSession } from "@/state/session";

export function App() {
  return (
    <SessionProvider>
      <ToastProvider>
        <Shell />
      </ToastProvider>
    </SessionProvider>
  );
}

function Shell() {
  const session = usePortalSession();

  return (
    <SiteShell
      site="portal"
      brand={{ name: "BestCodex" }}
      nav={[{ label: "产品", href: siteUrl("codex") }]}
      account={{ status: session.status, email: session.profile?.email }}
      accountLinks={{ login: "/login", signup: "/signup", account: "/account" }}
      footerExtra={
        <span>
          OpenAI、Codex、Claude、Anthropic 为其各自所有者的商标，与本项目无从属关系。
        </span>
      }
    >
      <Routes>
        <Route path="/" element={<Home />} />
        <Route path="/login" element={<Login />} />
        <Route path="/signup" element={<Signup />} />
        {/* 邀请链接的入口别名：?aff= 归因码在注册页被捕获（lib/affiliateRef）。 */}
        <Route path="/register" element={<Signup />} />
        <Route path="/account" element={<Account />} />
        <Route path="/authorize" element={<Authorize />} />
        <Route path="/logout" element={<Logout />} />
        <Route path="/help" element={<HelpIndex />} />
        <Route path="/help/:slug" element={<HelpArticle />} />
        <Route path="/codex" element={<ProductRedirect path="/codex" label="Codex" />} />
        <Route path="/claude" element={<ProductRedirect path="/claude" label="Claude" />} />
        <Route path="*" element={<NotFound />} />
      </Routes>
    </SiteShell>
  );
}
