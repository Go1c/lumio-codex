/**
 * 邀请返利归因码的落地与读取。
 *
 * 邀请链接形如 `https://bestcodex.app/register?aff=ABC123`（兼容 `aff_code` 参数）。
 * 归因码不是用户凭据：存 sessionStorage 只为让用户从邀请页逛到注册页（先看协议、
 * 去邮箱拿验证码再回来）之间不丢；注册提交时原样放进 `aff_code`，由 Sub2API
 * 大写化并静默绑定——绑定失败也不阻断注册（docs/ops/05 §2.2）。
 */

const STORAGE_KEY = "lumio_aff_ref";
const QUERY_KEYS = ["aff", "aff_code"] as const;

function normalize(value: string | null): string {
  const code = (value ?? "").trim();
  return /^[A-Za-z0-9_-]{4,32}$/.test(code) ? code : "";
}

/** 从 URL 取归因码（优先）并落地；URL 没有时回落 sessionStorage 的上次记录。 */
export function readAffiliateRef(params: URLSearchParams): string {
  for (const key of QUERY_KEYS) {
    const fromUrl = normalize(params.get(key));
    if (fromUrl) {
      try {
        sessionStorage.setItem(STORAGE_KEY, fromUrl);
      } catch {
        // 隐私模式等存储不可用：仅本次会话内生效即可。
      }
      return fromUrl;
    }
  }
  try {
    return normalize(sessionStorage.getItem(STORAGE_KEY));
  } catch {
    return "";
  }
}

/** 面向用户复制的邀请链接。 */
export function affiliateInviteLink(origin: string, code: string): string {
  return `${origin}/register?aff=${encodeURIComponent(code)}`;
}
