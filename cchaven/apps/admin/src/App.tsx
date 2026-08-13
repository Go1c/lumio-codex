import { Navigate, Route, Routes } from "react-router-dom";
import { AuthProvider, useAuth } from "./auth/AuthProvider";
import { LoginPage } from "./auth/LoginPage";
import { TotpChallenge } from "./auth/TotpChallenge";
import { TotpEnroll } from "./auth/TotpEnroll";
import { Sidebar } from "./components/Sidebar";
import { ToastProvider } from "./components/ToastProvider";
import { t } from "./i18n";
import { DashboardPage } from "./pages/DashboardPage";
import { ForbiddenPage } from "./pages/ForbiddenPage";
import { OrdersPage } from "./pages/OrdersPage";
import { SettingsPage } from "./pages/SettingsPage";
import { UserDetailPage } from "./pages/UserDetailPage";
import { UsersPage } from "./pages/UsersPage";

function Shell() {
  return (
    <div className="appframe">
      <Sidebar />
      <main className="app-main">
        <Routes>
          <Route path="/" element={<DashboardPage />} />
          <Route path="/users" element={<UsersPage />} />
          <Route path="/users/:userId" element={<UserDetailPage />} />
          <Route path="/orders" element={<OrdersPage />} />
          <Route path="/settings" element={<SettingsPage />} />
          <Route path="/403" element={<ForbiddenPage />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </main>
    </div>
  );
}

/** 会话状态决定整屏渲染什么：业务页面只有在完整会话（已过两步验证）下才会出现。 */
function Gate() {
  const { status } = useAuth();

  switch (status) {
    case "loading":
      return (
        <div className="boot" role="status">
          {t("common.loading")}
        </div>
      );
    case "anonymous":
      return <LoginPage />;
    case "mfa_challenge":
      return <TotpChallenge />;
    case "enroll":
      return <TotpEnroll />;
    case "forbidden":
      return <ForbiddenPage />;
    case "ready":
      return <Shell />;
  }
}

export function App() {
  return (
    <ToastProvider>
      <AuthProvider>
        <Gate />
      </AuthProvider>
    </ToastProvider>
  );
}
