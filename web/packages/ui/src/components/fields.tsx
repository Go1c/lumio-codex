import { useId, useState, type InputHTMLAttributes, type ReactNode } from "react";

interface TextFieldProps extends Omit<InputHTMLAttributes<HTMLInputElement>, "id"> {
  label: ReactNode;
  error?: ReactNode;
  hint?: ReactNode;
}

/** 字段级错误 inline 展示（红字 + 红边框），并用 aria-describedby 关联输入框。 */
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
        aria-describedby={
          [error ? errorId : null, hint ? hintId : null].filter(Boolean).join(" ") || undefined
        }
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

export function passwordRules(value: string) {
  return {
    hasLength: value.length >= 8,
    hasMix: /[A-Za-z]/.test(value) && /\d/.test(value),
  };
}

export function passwordStrength(value: string): 1 | 2 | 3 {
  const { hasLength, hasMix } = passwordRules(value);
  if (hasLength && hasMix && value.length >= 12) return 3;
  if (hasLength && hasMix) return 2;
  return 1;
}

/** 密码输入：显示 / 隐藏切换 + 强度条三档 + 规则清单实时打勾。 */
export function PasswordField({
  label = "密码",
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
  const id = useId();
  const [visible, setVisible] = useState(false);

  const { hasLength, hasMix } = passwordRules(value);
  const level = passwordStrength(value);
  const levelLabel = level >= 3 ? "强" : level === 2 ? "一般" : "弱";

  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
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
          aria-label={visible ? "隐藏密码" : "显示密码"}
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
          <p className="strength-label">密码强度：{levelLabel}</p>
          <ul className="rules">
            <li className={hasLength ? "ok" : ""}>
              <span aria-hidden="true">{hasLength ? "✓" : "○"}</span> 至少 8 个字符
            </li>
            <li className={hasMix ? "ok" : ""}>
              <span aria-hidden="true">{hasMix ? "✓" : "○"}</span> 包含字母和数字
            </li>
          </ul>
        </div>
      )}
    </div>
  );
}
