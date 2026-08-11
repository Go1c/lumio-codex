import type { FormEvent } from "react";
import { errorLiveRegionProps, type ScreenModel } from "./ui-model";

export type AccountFormProps = {
  screen: ScreenModel;
  errorMessage: string | null;
  onSubmit: (fields: Record<string, string>) => void;
  resendSeconds?: number;
  onResend?: () => void;
  longEmail?: string;
};

/**
 * Accessible account form shell used by register/login/verify/forgot/reset screens.
 */
export function AccountForm(props: AccountFormProps) {
  const { screen, errorMessage, onSubmit, resendSeconds, onResend, longEmail } = props;
  const live = errorLiveRegionProps(screen.errorId, errorMessage);

  function handleSubmit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const fd = new FormData(e.currentTarget);
    const fields: Record<string, string> = {};
    for (const f of screen.fields) {
      fields[f.id] = String(fd.get(f.id) ?? "");
    }
    onSubmit(fields);
  }

  return (
    <div className="account-shell">
      <div className="account-card">
        <h1>{screen.title}</h1>
        <p>{screen.description}</p>
        {longEmail ? (
          <p className={longEmail.length > 32 ? "email-field email-field--long" : "email-field"}>
            {longEmail}
          </p>
        ) : null}
        <form className="account-form" onSubmit={handleSubmit} noValidate>
          {screen.fields.map((f) => (
            <div key={f.id}>
              <label htmlFor={f.id}>{f.label}</label>
              <input
                id={f.id}
                name={f.id}
                type={f.type}
                autoComplete={f.autoComplete}
                required={f.required}
                aria-describedby={f.describedBy}
                inputMode={f.inputMode as "text" | "email" | "numeric" | undefined}
                maxLength={f.maxLength}
                pattern={f.pattern}
              />
            </div>
          ))}
          <div {...live} />
          <button type="submit">{screen.primaryAction}</button>
          {onResend ? (
            <button type="button" onClick={onResend} disabled={(resendSeconds ?? 0) > 0}>
              {(resendSeconds ?? 0) > 0 ? `Resend in ${resendSeconds}s` : "Resend code"}
            </button>
          ) : null}
          {screen.secondaryAction ? (
            <a href={screen.secondaryAction.href}>{screen.secondaryAction.label}</a>
          ) : null}
        </form>
      </div>
    </div>
  );
}
