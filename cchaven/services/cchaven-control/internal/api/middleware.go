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

type contextKey string

const (
	principalKey      contextKey = "principal"
	adminPrincipalKey contextKey = "admin_principal"
)

// requireUser 校验用户会话。APP 走 Authorization: Bearer，官网走 HttpOnly cookie。
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
			httpx.Fail(w, r, err)
			return
		}

		next.ServeHTTP(w, r.WithContext(context.WithValue(r.Context(), principalKey, principal)))
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
