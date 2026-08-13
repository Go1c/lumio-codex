package api

import (
	"context"
	"net/http"
	"net/url"
	"slices"
	"strings"
	"time"

	"github.com/google/uuid"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/apperr"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/httpx"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/service"
)

// sessionExpiredCode 是「本服务不认识这个令牌」的信号，requireUser 据它决定是否改问账号中心。
const sessionExpiredCode = "session_expired"

type contextKey string

const (
	principalKey      contextKey = "principal"
	adminPrincipalKey contextKey = "admin_principal"
)

// requireUser 校验调用者身份，接受两类令牌：
//
//  1. 本服务签发的 access token —— CC 桌面端经 OAuth 拿到的，本服务仍是它的
//     token issuer；存量官网 cookie 会话也走这条（新会话已无从建立）。
//  2. Sub2API 令牌 —— 统一门户与账号中心带着它来读 CC 的权益 / 邀请数据。
//
// 顺序是先本地后上游：本地校验不打外部网络，桌面端的高频心跳不该每次都惊动
// 账号中心。只有本地判定「这不是我签的」时才去 Sub2API 求证。
func (s *Server) requireUser(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		token, viaCookie := s.accessTokenFrom(r)
		if token == "" {
			httpx.Fail(w, r, apperr.Unauthorized())
			return
		}
		// Cookie 会被浏览器自动附带，因此写操作必须额外校验来源，防 CSRF。
		if viaCookie && !s.originAllowedFor(r) {
			httpx.Fail(w, r, apperr.Forbidden())
			return
		}

		principal, err := s.svc.AuthenticateAccess(r.Context(), token)
		if err != nil {
			// 只有「这不是我签的（或已作废）」才转问账号中心。
			// 账号被停用这类明确结论必须原样返回，不给第二次机会。
			if viaCookie || apperr.From(err).Code != sessionExpiredCode {
				httpx.Fail(w, r, err)
				return
			}
			principal, err = s.authenticateLumio(r, token)
			if err != nil {
				httpx.Fail(w, r, err)
				return
			}
		}

		next.ServeHTTP(w, r.WithContext(context.WithValue(r.Context(), principalKey, principal)))
	})
}

// requireLumioUser 只接受 Sub2API 令牌。
//
// 桌面授权用它：签发 CC 会话之前，必须先向账号中心确认「现在坐在浏览器前的是谁」，
// 拿本地会话冒名顶替是不允许的——本服务已经不是身份提供方了。
func (s *Server) requireLumioUser(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		token, viaCookie := s.accessTokenFrom(r)
		if token == "" || viaCookie {
			httpx.Fail(w, r, apperr.Unauthorized())
			return
		}

		principal, err := s.authenticateLumio(r, token)
		if err != nil {
			httpx.Fail(w, r, err)
			return
		}

		next.ServeHTTP(w, r.WithContext(context.WithValue(r.Context(), principalKey, principal)))
	})
}

// authenticateLumio 拿 Sub2API 令牌换本地身份。
//
// 邀请归因随请求一起带上：影子账号是在这一刻创建的，这也是本服务最后一次
// 还能看到 cch_ref cookie 的机会（注册已经不在这里发生了）。
func (s *Server) authenticateLumio(r *http.Request, token string) (service.Principal, error) {
	visitorID := s.referralVisitorFrom(r)
	return s.svc.AuthenticateSub2API(r.Context(), service.IdentityInput{
		Token:        token,
		ReferralCode: s.referralCodeFrom(r),
		VisitorID:    &visitorID,
		IP:           httpx.ClientIP(r),
		UserAgent:    httpx.UserAgent(r),
	})
}

// rateLimitPublic 对无需登录的只读接口按 IP 限频。
//
// 这些是除认证接口外仅有的匿名入口，此前完全没有配额。健康检查不在其列——
// 探针被限频会让编排系统误判服务不可用。
func (s *Server) rateLimitPublic(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if ok, retry := s.svc.Limiter.Allow(
			"public:"+httpx.ClientIP(r), service.RulePublicReadByIP,
		); !ok {
			httpx.Fail(w, r, apperr.RateLimited(retry))
			return
		}
		next.ServeHTTP(w, r)
	})
}

// requireAdmin 校验管理端会话；未通过两步验证的半会话会被拒绝。
func (s *Server) requireAdmin(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		token, viaCookie := s.adminTokenFrom(r)
		if viaCookie && !s.originAllowedFor(r) {
			httpx.Fail(w, r, apperr.Forbidden())
			return
		}

		principal, err := s.svc.AuthenticateAdmin(r.Context(), token)
		if err != nil {
			httpx.Fail(w, r, err)
			return
		}

		next.ServeHTTP(w, r.WithContext(context.WithValue(r.Context(), adminPrincipalKey, principal)))
	})
}

func principalOf(r *http.Request) service.Principal {
	principal, _ := r.Context().Value(principalKey).(service.Principal)
	return principal
}

func adminOf(r *http.Request) service.AdminPrincipal {
	principal, _ := r.Context().Value(adminPrincipalKey).(service.AdminPrincipal)
	return principal
}

// accessTokenFrom 提取 access token，并报告它是否来自 cookie。
func (s *Server) accessTokenFrom(r *http.Request) (token string, viaCookie bool) {
	if header := r.Header.Get("Authorization"); header != "" {
		if value, ok := strings.CutPrefix(header, "Bearer "); ok {
			return strings.TrimSpace(value), false
		}
	}
	if cookie, err := r.Cookie(s.cfg.CookieName.Session); err == nil {
		return cookie.Value, true
	}
	return "", false
}

func (s *Server) adminTokenFrom(r *http.Request) (token string, viaCookie bool) {
	if header := r.Header.Get("Authorization"); header != "" {
		if value, ok := strings.CutPrefix(header, "Bearer "); ok {
			return strings.TrimSpace(value), false
		}
	}
	if cookie, err := r.Cookie(s.cfg.CookieName.Admin); err == nil {
		return cookie.Value, true
	}
	return "", false
}

// originAllowedFor 对 cookie 鉴权的写操作做同源校验。
// 读操作（GET/HEAD）不改变状态，放行。
func (s *Server) originAllowedFor(r *http.Request) bool {
	if r.Method == http.MethodGet || r.Method == http.MethodHead || r.Method == http.MethodOptions {
		return true
	}

	origin := r.Header.Get("Origin")
	if origin == "" {
		// 无 Origin 头的多为服务端到服务端调用，此时必然携带 Bearer 而非 cookie。
		return false
	}
	return s.allowedOrigin(origin)
}

// allowedOrigin 判断来源是否可信。
//
// 可信来源是 config.Config.TrustedOrigins 这一个明确集合（官网 + 管理后台），
// CORS 响应头与 CSRF 同源校验共用它，不在这里另加特例。
// 开发环境额外放行本机任意端口，因为官网、后台与各种预览服务端口经常变。
func (s *Server) allowedOrigin(origin string) bool {
	if origin == "" {
		// 空 Origin 不可信：配置项缺省时也是空串，不能让两个空串撞成「同源」。
		return false
	}
	if slices.Contains(s.cfg.TrustedOrigins(), origin) {
		return true
	}
	if s.cfg.Env != "dev" {
		return false
	}

	parsed, err := url.Parse(origin)
	if err != nil {
		return false
	}
	host := parsed.Hostname()
	return host == "localhost" || host == "127.0.0.1"
}

// —— cookie 读写 ——

// setSessionCookies 写入官网会话 cookie。
//
// access token 与 refresh token 都放在 HttpOnly cookie 里，令牌完全不暴露给 JS，
// 从根上消除 XSS 窃取令牌的可能。
func (s *Server) setSessionCookies(w http.ResponseWriter, pair service.TokenPair) {
	s.writeCookie(w, s.cfg.CookieName.Session, pair.AccessToken, s.cfg.AccessTokenTTL)
	s.writeCookie(w, s.cfg.CookieName.Refresh, pair.RefreshToken, s.cfg.RefreshTokenTTL)
}

func (s *Server) clearSessionCookies(w http.ResponseWriter) {
	s.writeCookie(w, s.cfg.CookieName.Session, "", -time.Hour)
	s.writeCookie(w, s.cfg.CookieName.Refresh, "", -time.Hour)
}

func (s *Server) writeCookie(w http.ResponseWriter, name, value string, ttl time.Duration) {
	cookie := &http.Cookie{
		Name:     name,
		Value:    value,
		Path:     "/",
		HttpOnly: true,
		Secure:   s.cfg.SecureCookies,
		// 同站部署用 Lax 即可；控制面与前端不同站时必须配成 None，否则浏览器不发送 cookie。
		// 取值与 Secure 的联动都在 config 里裁决，这里只照做。
		SameSite: s.cfg.CookieSameSite,
		MaxAge:   int(ttl.Seconds()),
	}
	if ttl < 0 {
		cookie.MaxAge = -1
	}
	http.SetCookie(w, cookie)
}

// referralVisitorFrom 读取或新建邀请归因 cookie 中的访客 ID。
func (s *Server) referralVisitorFrom(r *http.Request) uuid.UUID {
	if cookie, err := r.Cookie(s.cfg.CookieName.Referral + "_vid"); err == nil {
		if id, err := uuid.Parse(cookie.Value); err == nil {
			return id
		}
	}
	return uuid.New()
}

// referralCodeFrom 读取邀请码 cookie。被邀请者注册时无需手动输入邀请码，就靠它。
func (s *Server) referralCodeFrom(r *http.Request) string {
	if cookie, err := r.Cookie(s.cfg.CookieName.Referral); err == nil {
		return cookie.Value
	}
	return ""
}

// setReferralCookies 下发邀请归因 cookie，有效期 30 天。
func (s *Server) setReferralCookies(w http.ResponseWriter, code string, visitorID uuid.UUID) {
	const ttl = 30 * 24 * time.Hour
	s.writeCookie(w, s.cfg.CookieName.Referral, code, ttl)
	s.writeCookie(w, s.cfg.CookieName.Referral+"_vid", visitorID.String(), ttl)
}
