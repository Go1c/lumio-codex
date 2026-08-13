import { Route, Routes } from "react-router-dom";

import { SiteLayout } from "@/components/SiteLayout";
import { ToastProvider } from "@/components/Toast";
import { LangProvider } from "@/i18n";
import { Account } from "@/pages/account/Account";
import { Authorize } from "@/pages/Authorize";
import { Download } from "@/pages/Download";
import { ForgotPassword } from "@/pages/ForgotPassword";
import { Home } from "@/pages/Home";
import { InviteLandingPage } from "@/pages/InviteLanding";
import { Login } from "@/pages/Login";
import { NotFound } from "@/pages/NotFound";
import { Pricing } from "@/pages/Pricing";
import { ResetPassword } from "@/pages/ResetPassword";
import { Signup } from "@/pages/Signup";
import { VerifyEmail } from "@/pages/VerifyEmail";
import { InviteAttributionProvider } from "@/state/inviteAttribution";
import { PublicConfigProvider } from "@/state/publicConfig";
import { SessionProvider } from "@/state/session";

export function App() {
  return (
    <LangProvider>
      <SessionProvider>
        <PublicConfigProvider>
          <InviteAttributionProvider>
            <ToastProvider>
              <Routes>
                <Route element={<SiteLayout />}>
                  <Route path="/" element={<Home />} />
                  <Route path="/i/:code" element={<InviteLandingPage />} />
                  <Route path="/pricing" element={<Pricing />} />
                  <Route path="/download" element={<Download />} />
                  <Route path="/signup" element={<Signup />} />
                  <Route path="/verify-email" element={<VerifyEmail />} />
                  <Route path="/login" element={<Login />} />
                  <Route path="/forgot-password" element={<ForgotPassword />} />
                  <Route path="/reset-password" element={<ResetPassword />} />
                  <Route path="/account" element={<Account />} />
                  <Route path="/authorize" element={<Authorize />} />
                  <Route path="*" element={<NotFound />} />
                </Route>
              </Routes>
            </ToastProvider>
          </InviteAttributionProvider>
        </PublicConfigProvider>
      </SessionProvider>
    </LangProvider>
  );
}
