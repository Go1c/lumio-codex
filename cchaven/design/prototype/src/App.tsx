import { Routes, Route } from "react-router-dom";
import { DemoProvider } from "./demo";
import SiteLayout from "./site/SiteLayout";
import { Home, Pricing, Download, InviteLanding } from "./site/pages";
import { Signup, VerifyEmail, Login, ForgotPassword, ResetPassword } from "./site/auth";
import SiteAccount from "./site/Account";
import AppShell from "./app/AppShell";
import AdminShell from "./admin/Admin";

export default function App() {
  return (
    <DemoProvider>
      <Routes>
        <Route element={<SiteLayout />}>
          <Route path="/" element={<Home />} />
          <Route path="/i/:code" element={<InviteLanding />} />
          <Route path="/pricing" element={<Pricing />} />
          <Route path="/download" element={<Download />} />
          <Route path="/signup" element={<Signup />} />
          <Route path="/verify-email" element={<VerifyEmail />} />
          <Route path="/login" element={<Login />} />
          <Route path="/forgot-password" element={<ForgotPassword />} />
          <Route path="/reset-password" element={<ResetPassword />} />
          <Route path="/account" element={<SiteAccount />} />
        </Route>
        <Route path="/app/*" element={<AppShell />} />
        <Route path="/admin/*" element={<AdminShell />} />
      </Routes>
    </DemoProvider>
  );
}
