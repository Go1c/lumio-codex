import { useState } from "react";
import { AccountForm } from "./features/auth/AccountForm";
import {
  forgotFlow,
  loginFlow,
  registerFlow,
  resendFlow,
  resetFlow,
  verifyFlow,
} from "./features/auth/flows";
import {
  forgotScreen,
  loginScreen,
  registerScreen,
  resetScreen,
  verifyScreen,
} from "./features/auth/ui-model";

type Screen = "register" | "verify" | "login" | "forgot" | "reset";

export default function App() {
  const [screen, setScreen] = useState<Screen>("register");
  const [error, setError] = useState<string | null>(null);
  const [email, setEmail] = useState("");
  const [userId, setUserId] = useState("");
  const [resendSeconds, setResendSeconds] = useState(0);

  async function onSubmit(fields: Record<string, string>) {
    setError(null);
    if (screen === "register") {
      const r = await registerFlow(fields.email, fields.password);
      if (!r.ok) {
        setError(r.message);
        return;
      }
      setEmail(fields.email);
      setScreen("verify");
      return;
    }
    if (screen === "verify") {
      const r = await verifyFlow(userId || email, fields.code);
      // mock verify uses email; keep flow
      if (!r.ok) {
        setError(r.message);
        return;
      }
      setScreen("login");
      return;
    }
    if (screen === "login") {
      const r = await loginFlow(fields.email, fields.password);
      if (!r.ok) {
        setError(r.message);
        return;
      }
      return;
    }
    if (screen === "forgot") {
      const r = await forgotFlow(fields.email);
      if (!r.ok) {
        setError(r.message);
        return;
      }
      setEmail(fields.email);
      setScreen("reset");
      return;
    }
    if (screen === "reset") {
      const r = await resetFlow(email, fields.code, fields.password);
      if (!r.ok) {
        setError(r.message);
        return;
      }
      setScreen("login");
    }
    void userId;
  }

  async function onResend() {
    const r = await resendFlow(email);
    if (!r.ok) {
      setError(r.message);
      if (r.retryAfterSeconds) setResendSeconds(r.retryAfterSeconds);
      return;
    }
    setResendSeconds(60);
  }

  const model =
    screen === "register"
      ? registerScreen
      : screen === "verify"
        ? verifyScreen
        : screen === "login"
          ? loginScreen
          : screen === "forgot"
            ? forgotScreen
            : resetScreen;

  return (
    <AccountForm
      screen={model}
      errorMessage={error}
      onSubmit={onSubmit}
      resendSeconds={screen === "verify" ? resendSeconds : undefined}
      onResend={screen === "verify" ? onResend : undefined}
      longEmail={email}
    />
  );
}
