/** 交互设计 6.1 节：密码 ≥8 位且同时含字母与数字。与服务端 security.ValidatePassword 一致。 */
export function passwordRules(value: string) {
  return {
    hasLength: value.length >= 8,
    hasMix: /[a-zA-Z]/.test(value) && /\d/.test(value),
  };
}

export function passwordValid(value: string): boolean {
  const { hasLength, hasMix } = passwordRules(value);
  return hasLength && hasMix;
}

export type PasswordStrength = 0 | 1 | 2 | 3;

/** 强度条三档：弱 / 一般 / 强。 */
export function passwordStrength(value: string): PasswordStrength {
  if (!value) return 0;
  const { hasLength, hasMix } = passwordRules(value);
  if (!hasLength) return 1;
  if (!hasMix) return 1;
  return value.length >= 12 ? 3 : 2;
}

export function emailValid(value: string): boolean {
  return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(value.trim());
}
