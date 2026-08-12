/**
 * Account auth UI state machine — pure transitions for register/verify/login/forgot/reset.
 */

export type AuthScreen =
  | "register"
  | "verify"
  | "login"
  | "forgot"
  | "reset"
  | "success";

/** Presentational UI status for forms and banners. */
export type UiStatus =
  | "idle"
  | "empty"
  | "loading"
  | "error"
  | "disabled"
  | "expired"
  | "rate_limited"
  | "offline"
  | "success";

export type AuthContext = {
  screen: AuthScreen;
  status: UiStatus;
  email: string;
  message: string | null;
  /** Seconds remaining before resend is allowed (0 = can resend). */
  resendSeconds: number;
  /** True while a submit is in flight. */
  submitting: boolean;
  /** Offline flag (navigator / transport). */
  offline: boolean;
};

export type AuthEvent =
  | { type: "NAVIGATE"; screen: AuthScreen }
  | { type: "SET_EMAIL"; email: string }
  | { type: "SUBMIT" }
  | { type: "SUCCESS"; message?: string; next?: AuthScreen }
  | { type: "FAIL"; status: Exclude<UiStatus, "idle" | "loading" | "success" | "empty">; message: string; retryAfterSeconds?: number }
  | { type: "TICK"; elapsedSeconds?: number }
  | { type: "RESEND_START"; retryAfterSeconds: number }
  | { type: "SET_OFFLINE"; offline: boolean }
  | { type: "RESET_STATUS" };

export function createInitialAuthContext(screen: AuthScreen = "login"): AuthContext {
  return {
    screen,
    status: "idle",
    email: "",
    message: null,
    resendSeconds: 0,
    submitting: false,
    offline: false,
  };
}

export function canSubmit(ctx: AuthContext): boolean {
  if (ctx.submitting || ctx.status === "loading" || ctx.status === "disabled") return false;
  if (ctx.offline || ctx.status === "offline") return false;
  return true;
}

export function canResend(ctx: AuthContext): boolean {
  if (!canSubmit(ctx)) return false;
  if (ctx.screen !== "verify" && ctx.screen !== "reset") return false;
  return ctx.resendSeconds <= 0;
}

export function reduceAuth(ctx: AuthContext, event: AuthEvent): AuthContext {
  switch (event.type) {
    case "NAVIGATE":
      return {
        ...createInitialAuthContext(event.screen),
        email: ctx.email,
        offline: ctx.offline,
        status: ctx.offline ? "offline" : "idle",
        message: ctx.offline ? "You appear to be offline. Check your connection and try again." : null,
      };
    case "SET_EMAIL":
      return { ...ctx, email: event.email };
    case "SUBMIT":
      if (!canSubmit(ctx)) return ctx;
      return { ...ctx, submitting: true, status: "loading", message: null };
    case "SUCCESS":
      return {
        ...ctx,
        submitting: false,
        status: "success",
        message: event.message ?? null,
        screen: event.next ?? ctx.screen,
        resendSeconds: event.next === "verify" || event.next === "reset" ? ctx.resendSeconds : 0,
      };
    case "FAIL": {
      const resend =
        event.status === "rate_limited" && event.retryAfterSeconds != null
          ? Math.max(0, Math.ceil(event.retryAfterSeconds))
          : ctx.resendSeconds;
      return {
        ...ctx,
        submitting: false,
        status: event.status,
        message: event.message,
        resendSeconds: resend,
      };
    }
    case "TICK": {
      if (ctx.resendSeconds <= 0) return ctx;
      const step = event.elapsedSeconds ?? 1;
      const next = Math.max(0, ctx.resendSeconds - step);
      return {
        ...ctx,
        resendSeconds: next,
        status: next === 0 && ctx.status === "rate_limited" ? "idle" : ctx.status,
        message: next === 0 && ctx.status === "rate_limited" ? null : ctx.message,
      };
    }
    case "RESEND_START":
      return {
        ...ctx,
        resendSeconds: Math.max(0, Math.ceil(event.retryAfterSeconds)),
        status: "rate_limited",
        message: event.retryAfterSeconds > 0 ? "Please wait before requesting another code." : ctx.message,
      };
    case "SET_OFFLINE":
      return {
        ...ctx,
        offline: event.offline,
        status: event.offline ? "offline" : ctx.status === "offline" ? "idle" : ctx.status,
        message: event.offline
          ? "You appear to be offline. Check your connection and try again."
          : ctx.status === "offline"
            ? null
            : ctx.message,
        submitting: event.offline ? false : ctx.submitting,
      };
    case "RESET_STATUS":
      return {
        ...ctx,
        status: ctx.offline ? "offline" : "idle",
        message: ctx.offline ? ctx.message : null,
        submitting: false,
      };
    default:
      return ctx;
  }
}

/** Map API / transport failures into machine FAIL events. */
export function failEventFromKind(
  kind: string,
  message: string,
  retryAfterSeconds?: number,
): Extract<AuthEvent, { type: "FAIL" }> {
  const statusMap: Record<string, Extract<AuthEvent, { type: "FAIL" }>["status"]> = {
    challenge_expired: "expired",
    expired: "expired",
    challenge_rate_limited: "rate_limited",
    rate_limited: "rate_limited",
    offline: "offline",
    disabled: "disabled",
    account_disabled: "disabled",
    validation: "error",
    invalid_credentials: "error",
    challenge_invalid: "error",
    challenge_exhausted: "error",
    email_not_verified: "error",
    account_locked: "error",
    server: "error",
    unknown: "error",
  };
  return {
    type: "FAIL",
    status: statusMap[kind] ?? "error",
    message,
    retryAfterSeconds,
  };
}
