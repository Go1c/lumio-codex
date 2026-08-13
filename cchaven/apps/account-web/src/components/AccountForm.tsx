/**
 * Presentational account form component (React).
 * Consumes pure flow controllers + state machine; no business logic here.
 */

import { useEffect, useMemo, useRef, useState, type FormEvent, type ClipboardEvent } from "react";
import {
  emailLayoutClass,
  errorLiveRegionProps,
  forgotScreen,
  initialFocusFieldId,
  loginScreen,
  registerScreen,
  resendControlProps,
  resetScreen,
  statusBannerClass,
  submitControlProps,
  verifyScreen,
  type ScreenModel,
} from "../features/auth/ui-model.js";
import {
  canResend,
  canSubmit,
  createInitialAuthContext,
  failEventFromKind,
  reduceAuth,
  type AuthContext,
  type AuthScreen,
} from "../features/auth/state-machine.js";
import {
  extractCodeFromPaste,
  forgotFlow,
  loginFlow,
  registerFlow,
  resendCountdown,
  resendFlow,
  resetFlow,
  verifyFlow,
  type FlowResult,
} from "../features/auth/flows.js";

export type AccountFormProps = {
  initialScreen?: AuthScreen;
  /** Optional external email prefill (e.g. after register → verify). */
  email?: string;
  onComplete?: (result: { screen: AuthScreen; data?: Record<string, unknown> }) => void;
};

type FormValues = Record<string, string>;

function emptyValues(screen: ScreenModel): FormValues {
  const v: FormValues = {};
  for (const f of screen.fields) v[f.id] = "";
  return v;
}

function screenFor(id: AuthScreen): ScreenModel | null {
  switch (id) {
    case "register":
      return registerScreen;
    case "verify":
      return verifyScreen;
    case "login":
      return loginScreen;
    case "forgot":
      return forgotScreen;
    case "reset":
      return resetScreen;
    default:
      return null;
  }
}

async function runPrimary(
  screen: AuthScreen,
  email: string,
  values: FormValues,
): Promise<FlowResult> {
  switch (screen) {
    case "register":
      return registerFlow(values.email ?? email, values.password ?? "");
    case "verify":
      return verifyFlow(email, values.code ?? "");
    case "login":
      return loginFlow(values.email ?? email, values.password ?? "");
    case "forgot":
      return forgotFlow(values.email ?? email);
    case "reset":
      return resetFlow(email, values.code ?? "", values.password ?? "");
    default:
      return { ok: false, kind: "unknown", message: "Unsupported screen." };
  }
}

export function AccountForm({
  initialScreen = "login",
  email: emailProp = "",
  onComplete,
}: AccountFormProps) {
  const [ctx, setCtx] = useState<AuthContext>(() => {
    const base = createInitialAuthContext(initialScreen);
    return { ...base, email: emailProp };
  });
  const [values, setValues] = useState<FormValues>({});
  const formRef = useRef<HTMLFormElement>(null);
  const codeInputRef = useRef<HTMLInputElement>(null);

  const screenModel = useMemo(() => screenFor(ctx.screen), [ctx.screen]);

  useEffect(() => {
    if (!screenModel) return;
    setValues(emptyValues(screenModel));
    const focusId = initialFocusFieldId(screenModel, false);
    queueMicrotask(() => {
      const el = formRef.current?.querySelector<HTMLElement>(`#${CSS.escape(focusId)}`);
      el?.focus();
    });
  }, [screenModel]);

  // Resend countdown ticker
  useEffect(() => {
    if (ctx.resendSeconds <= 0) return;
    const id = setInterval(() => {
      setCtx((c) => reduceAuth(c, { type: "TICK", elapsedSeconds: 1 }));
    }, 1000);
    return () => clearInterval(id);
  }, [ctx.resendSeconds > 0]);

  if (!screenModel) {
    return (
      <div className="account-shell" data-status="success">
        <div className="account-card">
          <h1>Success</h1>
          <p role="status">{ctx.message ?? "You are signed in."}</p>
        </div>
      </div>
    );
  }

  const submitProps = submitControlProps(ctx.status, ctx.submitting);
  const resendProps = resendControlProps(ctx.resendSeconds, ctx.status, ctx.submitting);
  const live = errorLiveRegionProps(screenModel.errorId, ctx.message);
  const emailClass = emailLayoutClass(ctx.email || values.email || "");

  async function onSubmit(e: FormEvent) {
    e.preventDefault();
    if (!canSubmit(ctx) || !screenModel) return;
    setCtx((c) => reduceAuth(c, { type: "SUBMIT" }));
    const email = (values.email || ctx.email || emailProp).trim();
    const result = await runPrimary(ctx.screen, email, values);
    if (result.ok) {
      const rawNext = result.next || ctx.screen;
      const nextScreen: AuthScreen = rawNext === "home" ? "success" : (rawNext as AuthScreen);
      setCtx((c) => {
        let n = reduceAuth(c, {
          type: "SUCCESS",
          message: typeof result.data?.message === "string" ? result.data.message : undefined,
          next: nextScreen,
        });
        if (result.data?.email && typeof result.data.email === "string") {
          n = reduceAuth(n, { type: "SET_EMAIL", email: result.data.email });
        }
        if (typeof result.data?.resendAfterSeconds === "number") {
          n = {
            ...n,
            resendSeconds: resendCountdown(result.data.resendAfterSeconds as number, 0),
          };
        }
        return n;
      });
      onComplete?.({ screen: nextScreen, data: result.data });
    } else {
      setCtx((c) =>
        reduceAuth(
          c,
          failEventFromKind(result.kind, result.message, result.retryAfterSeconds),
        ),
      );
    }
  }

  async function onResend() {
    if (!canResend(ctx)) return;
    setCtx((c) => reduceAuth(c, { type: "SUBMIT" }));
    const result = await resendFlow(ctx.email || values.email || emailProp);
    if (result.ok) {
      const seconds =
        typeof result.data?.resendAfterSeconds === "number"
          ? (result.data.resendAfterSeconds as number)
          : 60;
      setCtx((c) => reduceAuth(c, { type: "RESEND_START", retryAfterSeconds: seconds }));
    } else {
      setCtx((c) =>
        reduceAuth(
          c,
          failEventFromKind(result.kind, result.message, result.retryAfterSeconds),
        ),
      );
    }
  }

  function onPasteCode(ev: ClipboardEvent<HTMLInputElement>) {
    const text = ev.clipboardData.getData("text");
    const code = extractCodeFromPaste(text);
    if (code) {
      ev.preventDefault();
      setValues((v) => ({ ...v, code }));
      codeInputRef.current?.focus();
    }
  }

  return (
    <div className="account-shell" data-status={ctx.status}>
      <div className={`account-card ${statusBannerClass(ctx.status)}`}>
        <h1>{screenModel.title}</h1>
        <p className="account-description">{screenModel.description}</p>
        <form ref={formRef} className="account-form" onSubmit={onSubmit} noValidate>
          {screenModel.fields.map((field) => {
            const isEmail = field.type === "email";
            const isCode = field.id === "code";
            return (
              <div key={field.id} className={isEmail ? emailClass : "field"}>
                <label htmlFor={field.id}>{field.label}</label>
                <input
                  ref={isCode ? codeInputRef : undefined}
                  id={field.id}
                  name={field.id}
                  type={field.type}
                  autoComplete={field.autoComplete}
                  required={field.required}
                  inputMode={field.inputMode as "numeric" | undefined}
                  maxLength={field.maxLength}
                  pattern={field.pattern}
                  aria-describedby={field.describedBy}
                  aria-invalid={ctx.status === "error" || ctx.status === "expired"}
                  value={values[field.id] ?? ""}
                  disabled={submitProps.disabled && ctx.status === "disabled"}
                  onChange={(e) => {
                    const v = e.target.value;
                    setValues((prev) => ({ ...prev, [field.id]: v }));
                    if (isEmail) {
                      setCtx((c) => reduceAuth(c, { type: "SET_EMAIL", email: v }));
                    }
                  }}
                  onPaste={field.pasteCode ? onPasteCode : undefined}
                />
              </div>
            );
          })}

          <div
            id={live.id}
            role={live.role}
            aria-live={live["aria-live"]}
            aria-atomic={live["aria-atomic"]}
            hidden={live.hidden}
            className="account-error"
          >
            {live.children}
          </div>

          <button type="submit" {...submitProps}>
            {ctx.submitting || ctx.status === "loading" ? "Working…" : screenModel.primaryAction}
          </button>

          {screenModel.showResend ? (
            <button type="button" className="account-resend" onClick={onResend} {...resendProps}>
              {resendProps.label}
            </button>
          ) : null}

          {screenModel.secondaryAction ? (
            <p className="account-secondary">
              <a href={screenModel.secondaryAction.href}>{screenModel.secondaryAction.label}</a>
            </p>
          ) : null}
        </form>
      </div>
    </div>
  );
}

export default AccountForm;
