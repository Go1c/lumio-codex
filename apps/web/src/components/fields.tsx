import { useId, useState, type InputHTMLAttributes, type ReactNode } from "react";

import { useT } from "@/i18n";
import { passwordRules, passwordStrength } from "@/lib/validation";

interface TextFieldProps extends Omit<InputHTMLAttributes<HTMLInputElement>, "id"> {
  label: ReactNode;
  error?: ReactNode;
  hint?: ReactNode;
}

/** 6.1 节：字段级错误 inline（红字 + 红边框，位于字段下方），并用 aria-describedby 关联。 */
export function TextField({ label, error, hint, className = "", ...inputProps }: TextFieldProps) {
  const id = useId();
  const errorId = `${id}-error`;
  const hintId = `${id}-hint`;

  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      <input
        {...inputProps}
        id={id}
        className={`${className} ${error ? "invalid" : ""}`.trim()}
        aria-invalid={error ? true : undefined}
        aria-describedby={[error ? errorId : null, hint ? hintId : null].filter(Boolean).join(" ") || undefined}
      />
      {hint && (
        <div className="hint" id={hintId}>
          {hint}
        </div>
      )}
      {error && (
        <div className="err" id={errorId}>
          {error}
        </div>
      )}
    </div>
  );
}

/**
 * 密码输入：显示/隐藏切换 + 强度条三档 + 规则清单实时打勾（4.5 / 6.1 节）。
 */
export function PasswordField({
  label,
  value,
  onChange,
  showRules = true,
  autoComplete = "new-password",
  error,
  required,
}: {
  label?: string;
  value: string;
  onChange: (value: string) => void;
  showRules?: boolean;
  autoComplete?: string;
  error?: ReactNode;
  required?: boolean;
}) {
  const t = useT();
  const id = useId();
  const [visible, setVisible] = useState(false);

  const { hasLength, hasMix } = passwordRules(value);
  const level = passwordStrength(value);
  const levelLabel =
    level >= 3 ? t("password.strength_strong") : level === 2 ? t("password.strength_fair") : t("password.strength_weak");

  return (
    <div className="field">
      <label htmlFor={id}>{label ?? t("password.label")}</label>
      <div className="password-input">
        <input
          id={id}
          type={visible ? "text" : "password"}
          value={value}
          required={required}
          autoComplete={autoComplete}
          onChange={(event) => onChange(event.target.value)}
          className={error ? "invalid" : ""}
          aria-invalid={error ? true : undefined}
          aria-describedby={showRules ? `${id}-rules` : undefined}
          placeholder="••••••••"
        />
        <button
          type="button"
          className="password-toggle"
          onClick={() => setVisible((v) => !v)}
          aria-label={visible ? t("password.hide") : t("password.show")}
          aria-pressed={visible}
        >
          {visible ? "🙈" : "👁"}
        </button>
      </div>
      {error && <div className="err">{error}</div>}
      {showRules && value.length > 0 && (
        <div id={`${id}-rules`}>
          <div className={`strength s${level}`} aria-hidden="true">
            <span />
            <span />
            <span />
          </div>
          <p className="strength-label">{t("password.strength", { level: levelLabel })}</p>
          <ul className="rules">
            <li className={hasLength ? "ok" : ""}>
              <span aria-hidden="true">{hasLength ? "✓" : "○"}</span> {t("password.rule_length")}
            </li>
            <li className={hasMix ? "ok" : ""}>
              <span aria-hidden="true">{hasMix ? "✓" : "○"}</span> {t("password.rule_mix")}
            </li>
          </ul>
        </div>
      )}
    </div>
  );
}
