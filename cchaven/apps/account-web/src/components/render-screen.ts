/**
 * DOM-free render helpers for account screens (used by tests & non-React hosts).
 * Produces a serializable view-model that mirrors AccountForm a11y attributes.
 */

import {
  emailLayoutClass,
  errorLiveRegionProps,
  initialFocusFieldId,
  resendControlProps,
  screens,
  statusBannerClass,
  submitControlProps,
  type ScreenModel,
} from "../features/auth/ui-model.js";
import type { AuthContext, AuthScreen } from "../features/auth/state-machine.js";
import { canResend, canSubmit } from "../features/auth/state-machine.js";

export type RenderedField = {
  id: string;
  label: string;
  type: string;
  autoComplete: string;
  required: boolean;
  inputMode?: string;
  maxLength?: number;
  pattern?: string;
  pasteCode?: boolean;
  className: string;
  value: string;
};

export type RenderedScreen = {
  screen: AuthScreen;
  title: string;
  description: string;
  statusClass: string;
  status: AuthContext["status"];
  fields: RenderedField[];
  error: ReturnType<typeof errorLiveRegionProps>;
  submit: ReturnType<typeof submitControlProps> & { label: string; canSubmit: boolean };
  resend?: ReturnType<typeof resendControlProps> & { canResend: boolean };
  secondaryAction?: ScreenModel["secondaryAction"];
  focusFieldId: string;
  minWidthPx: number;
};

export function renderScreen(
  ctx: AuthContext,
  values: Record<string, string> = {},
): RenderedScreen | null {
  const model = screens[ctx.screen];
  if (!model) return null;

  const fields: RenderedField[] = model.fields.map((f) => ({
    id: f.id,
    label: f.label,
    type: f.type,
    autoComplete: f.autoComplete,
    required: f.required,
    inputMode: f.inputMode,
    maxLength: f.maxLength,
    pattern: f.pattern,
    pasteCode: f.pasteCode,
    className: f.type === "email" ? emailLayoutClass(values.email ?? ctx.email ?? "") : "field",
    value: values[f.id] ?? "",
  }));

  const submitBase = submitControlProps(ctx.status, ctx.submitting);
  const rendered: RenderedScreen = {
    screen: ctx.screen,
    title: model.title,
    description: model.description,
    statusClass: statusBannerClass(ctx.status),
    status: ctx.status,
    fields,
    error: errorLiveRegionProps(model.errorId, ctx.message),
    submit: {
      ...submitBase,
      label: ctx.submitting || ctx.status === "loading" ? "Working…" : model.primaryAction,
      canSubmit: canSubmit(ctx),
    },
    secondaryAction: model.secondaryAction,
    focusFieldId: initialFocusFieldId(model, Boolean(ctx.message)),
    minWidthPx: 375,
  };

  if (model.showResend) {
    const r = resendControlProps(ctx.resendSeconds, ctx.status, ctx.submitting);
    rendered.resend = { ...r, canResend: canResend(ctx) };
  }

  return rendered;
}

/** Assert layout rules for long emails at 375px (string-level CSS contract). */
export function longEmailLayoutOk(email: string, cssText: string): boolean {
  if (email.length <= 32) return true;
  const hasWrap =
    cssText.includes("overflow-wrap") ||
    cssText.includes("word-break") ||
    cssText.includes("email-field--long");
  const noPageScroll = cssText.includes("overflow-x: hidden") || cssText.includes("max-width: 100%");
  return hasWrap && noPageScroll;
}
