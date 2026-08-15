export const LUMIO_ERROR_COPY: Record<string, string> = {
  AUTH_INVALID_CREDENTIALS: "邮箱或密码不正确",
  AUTH_CODE_INVALID: "验证码不正确或已过期",
  AUTH_CODE_REQUIRED: "请先获取邮箱验证码",
  AUTH_CODE_RATE_LIMITED: "发送太频繁，请稍后再试",
  AUTH_EMAIL_DOMAIN_NOT_ALLOWED: "该邮箱后缀暂不支持",
  AUTH_EMAIL_ALREADY_REGISTERED: "该邮箱已注册，请直接登录",
  AUTH_EMAIL_INVALID: "请填写有效的邮箱地址",
  AUTH_INVITATION_CODE_REQUIRED: "注册需要邀请码，请填写后重试",
  AUTH_INVITATION_CODE_INVALID: "邀请码无效或已被使用",
  AUTH_REGISTRATION_CLOSED: "注册暂未开放",
  AUTH_2FA_INVALID: "两步验证码不正确",
  AUTH_2FA_UNAVAILABLE: "两步验证当前不可用，请联系支持",
  AUTH_ACCOUNT_DISABLED: "该账户已被停用",
  AUTH_SESSION_EXPIRED: "登录已过期，请重新登录",
  ACCOUNT_INSUFFICIENT_BALANCE: "账户余额不足，请先充值",
  KEY_PROVISION_FAILED: "连接初始化失败，可重试",
  KEY_STORAGE_UNAVAILABLE: "无法访问系统安全存储",
  SERVICE_UNAVAILABLE: "服务暂时不可用，稍后自动重试",
  SERVICE_MODEL_CATALOG_EMPTY: "当前没有可用模型，请稍后重试或联系支持",
  SERVICE_RATE_LIMITED: "请求过于频繁，请稍后再试",
  SERVICE_VERSION_TOO_OLD: "当前版本过旧，请更新后继续",
  CODEX_APP_NOT_FOUND: "未检测到官方应用",
  CODEX_APP_INSTALL_FAILED: "安装官方应用失败，可重试",
  CODEX_APP_DOWNLOAD_FAILED: "下载官方应用失败，请检查网络后重试",
  CODEX_APP_VERIFY_FAILED: "官方应用校验未通过，已放弃安装",
  CODEX_APP_INVALID: "所选应用无法识别为官方 Codex",
  CODEX_CONFIG_CONFLICT: "检测到本机配置被其他工具修改过",
  CODEX_CONFIG_WRITE_FAILED: "写入本机配置失败，已保留原始内容",
  CODEX_RESTORE_FAILED: "恢复未完成，已保留原始快照",
  CODEX_LAUNCH_FAILED: "启动官方 Codex 失败",
  PAYMENT_HANDOFF_CREATE_FAILED: "暂时无法发起充值",
  PAYMENT_HANDOFF_EXPIRED: "支付链接已过期，请重新打开",
  PREFERENCE_LAUNCH_AT_LOGIN_FAILED: "开机启动设置未生效，请稍后重试",
  PREFERENCE_LAUNCH_AT_LOGIN_UNSUPPORTED: "当前运行方式不支持开机启动",
  UPDATE_VERIFY_FAILED: "更新包校验未通过，已放弃安装",
  UNKNOWN: "出现未知问题，请稍后重试",
};

const UNKNOWN_CODE = "UNKNOWN";

function normalizeCode(code: string | null | undefined): string {
  if (typeof code !== "string") return UNKNOWN_CODE;
  const trimmed = code.trim();
  if (trimmed === "" || !Object.hasOwn(LUMIO_ERROR_COPY, trimmed)) return UNKNOWN_CODE;
  return trimmed;
}

export function lumioErrorCopy(code: string | null | undefined): string {
  return LUMIO_ERROR_COPY[normalizeCode(code)];
}

export function lumioErrorLabel(code: string | null | undefined): string {
  const resolved = normalizeCode(code);
  return `${LUMIO_ERROR_COPY[resolved]}（${resolved}）`;
}
