import { Route, Routes } from "react-router-dom";

import { SiteShell, ToastProvider, siteUrl } from "@lumio/ui";

import { Account } from "@/pages/Account";
import { Home } from "@/pages/Home";
import { Login } from "@/pages/Login";
import { Logout } from "@/pages/Logout";
import { NotFound } from "@/pages/NotFound";
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
      brand={{ name: "Lumio" }}
      nav={[
        { label: "Lumio Codex", href: siteUrl("codex") },
        { label: "CC避风港", href: siteUrl("cc") },
      ]}
      account={{ status: session.status, email: session.profile?.email }}
      accountLinks={{ login: "/login", signup: "/signup", account: "/account" }}
    >
      <Routes>
        <Route path="/" element={<Home />} />
        <Route path="/login" element={<Login />} />
        <Route path="/signup" element={<Signup />} />
        <Route path="/account" element={<Account />} />
        <Route path="/logout" element={<Logout />} />
        <Route path="*" element={<NotFound />} />
      </Routes>
    </SiteShell>
  );
}
