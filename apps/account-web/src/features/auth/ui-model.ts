/**
 * Presentational model for account forms (a11y + responsive + long-email).
 */

import type { AuthScreen, UiStatus } from "./state-machine.js";

export type FieldModel = {
  id: string;
  label: string;
  type: "email" | "password" | "text";
  autoComplete: string;
  required: boolean;
  describedBy?: string;
  inputMode?: string;
  maxLength?: number;
  pattern?: string;
  /** When true, paste handler should extract challenge code. */
  pasteCode?: boolean;
};

export type ScreenModel = {
  id: AuthScreen;
  title: string;
  description: string;
  fields: FieldModel[];
  primaryAction: string;
  secondaryAction?: { label: string; href: string };
  errorId: string;
  showResend?: boolean;
};

export const registerScreen: ScreenModel = {
  id: "register",
  title: "Create account",
  description: "Register with your email. We will send a verification code.",
  errorId: "register-error",
  primaryAction: "Register",
  secondaryAction: { label: "Sign in", href: "/login" },
  fields: [
    {
      id: "email",
      label: "Email",
      type: "email",
      autoComplete: "email",
      required: true,
      describedBy: "register-error",
    },
    {
      id: "password",
      label: "Password",
      type: "password",
      autoComplete: "new-password",
      required: true,
      describedBy: "register-error",
    },
  ],
};

export const verifyScreen: ScreenModel = {
  id: "verify",
  title: "Verify email",
  description: "Enter the 6-digit code from your email. You can paste the whole message.",
  errorId: "verify-error",
  primaryAction: "Verify",
  showResend: true,
  fields: [
    {
      id: "code",
      label: "Verification code",
      type: "text",
      autoComplete: "one-time-code",
      required: true,
      inputMode: "numeric",
      maxLength: 6,
      pattern: "\\d{6}",
      describedBy: "verify-error",
      pasteCode: true,
    },
  ],
};

export const loginScreen: ScreenModel = {
  id: "login",
  title: "Sign in",
  description: "Sign in with your email and password.",
  errorId: "login-error",
  primaryAction: "Sign in",
  secondaryAction: { label: "Forgot password", href: "/forgot" },
  fields: [
    {
      id: "email",
      label: "Email",
      type: "email",
      autoComplete: "username",
      required: true,
      describedBy: "login-error",
    },
    {
      id: "password",
      label: "Password",
      type: "password",
      autoComplete: "current-password",
      required: true,
      describedBy: "login-error",
    },
  ],
};

export const forgotScreen: ScreenModel = {
  id: "forgot",
  title: "Forgot password",
  description:
    "If an account exists for this email, further instructions were sent or credentials were checked.",
  errorId: "forgot-error",
  primaryAction: "Send reset code",
  secondaryAction: { label: "Back to sign in", href: "/login" },
  fields: [
    {
      id: "email",
      label: "Email",
      type: "email",
      autoComplete: "email",
      required: true,
      describedBy: "forgot-error",
    },
  ],
};

export const resetScreen: ScreenModel = {
  id: "reset",
  title: "Reset password",
  description: "Enter the code from your email and choose a new password.",
  errorId: "reset-error",
  primaryAction: "Reset password",
  showResend: true,
  fields: [
    {
      id: "code",
      label: "Reset code",
      type: "text",
      autoComplete: "one-time-code",
      required: true,
      inputMode: "numeric",
      maxLength: 6,
      pattern: "\\d{6}",
      describedBy: "reset-error",
      pasteCode: true,
    },
    {
      id: "password",
      label: "New password",
      type: "password",
      autoComplete: "new-password",
      required: true,
      describedBy: "reset-error",
    },
  ],
};

export const screens: Record<AuthScreen, ScreenModel | null> = {
  register: registerScreen,
  verify: verifyScreen,
  login: loginScreen,
  forgot: forgotScreen,
  reset: resetScreen,
  success: null,
};

/** Long email must not force horizontal page scroll at 375px. */
export function emailLayoutClass(email: string): string {
  return email.length > 32 ? "email-field email-field--long" : "email-field";
}

export function minViewportWidth(): number {
  return 375;
}

/** Focus management: first invalid or first field. */
export function initialFocusFieldId(screen: ScreenModel, hasError: boolean): string {
  if (hasError) return screen.errorId;
  return screen.fields[0]?.id ?? screen.errorId;
}

/** Aria attributes for error live region. */
export function errorLiveRegionProps(errorId: string, message: string | null) {
  return {
    id: errorId,
    role: "alert" as const,
    "aria-live": "assertive" as const,
    "aria-atomic": true,
    hidden: !message,
    children: message ?? "",
  };
}

/** Map machine status → button disabled / busy attributes. */
export function submitControlProps(status: UiStatus, submitting: boolean) {
  const disabled =
    submitting ||
    status === "loading" ||
    status === "disabled" ||
    status === "offline";
  return {
    disabled,
    "aria-busy": submitting || status === "loading",
    "data-status": status,
  };
}

export function resendControlProps(resendSeconds: number, status: UiStatus, submitting: boolean) {
  const disabled = submitting || resendSeconds > 0 || status === "offline" || status === "disabled";
  return {
    disabled,
    "aria-disabled": disabled,
    "data-countdown": resendSeconds,
    label: resendSeconds > 0 ? `Resend code in ${resendSeconds}s` : "Resend code",
  };
}

/** Status banner class for CSS. */
export function statusBannerClass(status: UiStatus): string {
  return `account-status account-status--${status}`;
}
