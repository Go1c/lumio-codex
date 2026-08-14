package api

import (
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/config"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/ratelimit"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/service"
)

const (
	webOrigin   = "https://cchaven.cn"
	adminOrigin = "https://admin.cchaven.cn"
)

func prodConfig() config.Config {
	return config.Config{
		Env:            "prod",
		PublicURL:      webOrigin,
		AdminURL:       adminOrigin,
		SecureCookies:  true,
		CookieSameSite: http.SameSiteLaxMode,
	}
}

// TestAllowedOrigin 锁住可信来源集合。
//
// 这条用例是为一个真实事故写的：可信来源曾经只有 PublicURL 一个，
// 管理后台（独立部署在 admin.cchaven.cn）在生产环境拿不到 CORS 头，
// 每一个写操作还会被同源校验判成 403，而 dev 因为放行 localhost 完全看不出来。
func TestAllowedOrigin(t *testing.T) {
	cases := []struct {
		name   string
		env    string
		origin string
		want   bool
	}{
		{"生产环境放行官网", "prod", webOrigin, true},
		{"生产环境放行管理后台", "prod", adminOrigin, true},
		{"生产环境拒绝陌生来源", "prod", "https://evil.example.com", false},
		{"生产环境拒绝后台域名的仿冒前缀", "prod", "https://admin.cchaven.cn.evil.com", false},
		{"生产环境拒绝本机来源", "prod", "http://localhost:5183", false},
		{"生产环境拒绝空来源", "prod", "", false},
		{"开发环境放行官网端口", "dev", "http://localhost:5173", true},
		{"开发环境放行后台端口", "dev", "http://localhost:5183", true},
		{"开发环境放行回环地址", "dev", "http://127.0.0.1:8080", true},
		{"开发环境仍拒绝外部来源", "dev", "https://evil.example.com", false},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			cfg := prodConfig()
			cfg.Env = tc.env

			server := NewServer(nil, cfg)
			if got := server.allowedOrigin(tc.origin); got != tc.want {
				t.Errorf("allowedOrigin(%q) 在 %s 环境 = %v, want %v", tc.origin, tc.env, got, tc.want)
			}
		})
	}
}

// TestAllowedOriginWithoutAdminURL 验证未配置管理后台地址时不会把空 Origin 当成可信来源。
func TestAllowedOriginWithoutAdminURL(t *testing.T) {
	cfg := prodConfig()
	cfg.AdminURL = ""

	server := NewServer(nil, cfg)
	if server.allowedOrigin("") {
		t.Error("空 Origin 不能因为 AdminURL 也是空串就被判为可信")
	}
	if !server.allowedOrigin(webOrigin) {
		t.Error("官网来源应始终可信")
	}
}

// TestCORSHeadersForAdminOrigin 验证管理后台能拿到带凭证的 CORS 响应头。
func TestCORSHeadersForAdminOrigin(t *testing.T) {
	server := NewServer(nil, prodConfig())
	handler := server.cors(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusOK)
	}))

	for _, origin := range []string{webOrigin, adminOrigin} {
		req := httptest.NewRequest(http.MethodGet, "/api/admin/v1/auth/me", nil)
		req.Header.Set("Origin", origin)
		rec := httptest.NewRecorder()

		handler.ServeHTTP(rec, req)

		if got := rec.Header().Get("Access-Control-Allow-Origin"); got != origin {
			t.Errorf("来源 %s 的 Allow-Origin = %q, want %q", origin, got, origin)
		}
		if got := rec.Header().Get("Access-Control-Allow-Credentials"); got != "true" {
			t.Errorf("来源 %s 应允许携带凭证, got %q", origin, got)
		}
	}
}

// TestOriginAllowedForWriteFromAdmin 验证后台的写操作能通过 CSRF 同源校验。
func TestOriginAllowedForWriteFromAdmin(t *testing.T) {
	server := NewServer(nil, prodConfig())

	write := httptest.NewRequest(http.MethodPost, "/api/admin/v1/users/1/disable", nil)
	write.Header.Set("Origin", adminOrigin)
	if !server.originAllowedFor(write) {
		t.Error("管理后台的写操作不应被同源校验拦下")
	}

	forged := httptest.NewRequest(http.MethodPost, "/api/admin/v1/users/1/disable", nil)
	forged.Header.Set("Origin", "https://evil.example.com")
	if server.originAllowedFor(forged) {
		t.Error("第三方站点的写操作必须被拒绝")
	}
}

// TestWriteCookieSameSite 验证 cookie 的 SameSite 与 Secure 按配置下发。
func TestWriteCookieSameSite(t *testing.T) {
	cases := []struct {
		name       string
		cfg        config.Config
		wantSite   http.SameSite
		wantSecure bool
	}{
		{
			name:       "同站部署默认 Lax",
			cfg:        config.Config{CookieSameSite: http.SameSiteLaxMode, SecureCookies: true},
			wantSite:   http.SameSiteLaxMode,
			wantSecure: true,
		},
		{
			name:       "跨站部署下发 None",
			cfg:        config.Config{CookieSameSite: http.SameSiteNoneMode, SecureCookies: true},
			wantSite:   http.SameSiteNoneMode,
			wantSecure: true,
		},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			rec := httptest.NewRecorder()
			NewServer(nil, tc.cfg).writeCookie(rec, "cch_sess", "token", time.Hour)

			cookies := rec.Result().Cookies()
			if len(cookies) != 1 {
				t.Fatalf("应下发 1 个 cookie, got %d", len(cookies))
			}
			cookie := cookies[0]

			if cookie.SameSite != tc.wantSite {
				t.Errorf("SameSite = %v, want %v", cookie.SameSite, tc.wantSite)
			}
			if cookie.Secure != tc.wantSecure {
				t.Errorf("Secure = %v, want %v", cookie.Secure, tc.wantSecure)
			}
			if !cookie.HttpOnly {
				t.Error("会话 cookie 必须是 HttpOnly")
			}
		})
	}
}

// TestRateLimitPublic 锁住公开只读接口的按 IP 配额。
//
// 价格配置、邀请落地与归因回执是除认证接口外仅有的匿名入口，此前完全没有配额。
func TestRateLimitPublic(t *testing.T) {
	server := NewServer(&service.Service{Limiter: ratelimit.New()}, prodConfig())

	var served int
	handler := server.rateLimitPublic(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		served++
		w.WriteHeader(http.StatusOK)
	}))

	call := func(ip string) *httptest.ResponseRecorder {
		req := httptest.NewRequest(http.MethodGet, "/api/v1/config/public", nil)
		req.Header.Set("X-Forwarded-For", ip)

		rec := httptest.NewRecorder()
		handler.ServeHTTP(rec, req)
		return rec
	}

	const visitor = "203.0.113.7"
	for i := range service.RulePublicReadByIP.Limit {
		if got := call(visitor).Code; got != http.StatusOK {
			t.Fatalf("配额内第 %d 次请求应放行, got %d", i+1, got)
		}
	}

	blocked := call(visitor)
	if blocked.Code != http.StatusTooManyRequests {
		t.Errorf("超出配额应返回 429, got %d", blocked.Code)
	}
	if served != service.RulePublicReadByIP.Limit {
		t.Errorf("被拒的请求不应到达 handler: 实际处理 %d 次, 配额 %d",
			served, service.RulePublicReadByIP.Limit)
	}

	// 配额按 IP 隔离，一个访客把额度用完不该影响其他人。
	if got := call("198.51.100.4").Code; got != http.StatusOK {
		t.Errorf("其他 IP 应不受影响, got %d", got)
	}
}

// TestRateLimitAdminTOTP 锁住管理端 TOTP 端点的按 IP 配额（QA S-1）。
//
// 口令锁定只按账号计数；没有这层按来源的限频，攻击者仍可换着账号/会话
// 对 6 位验证码做在线穷举。
func TestRateLimitAdminTOTP(t *testing.T) {
	server := NewServer(&service.Service{Limiter: ratelimit.New()}, prodConfig())

	var served int
	handler := server.rateLimitAdminTOTP(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		served++
		w.WriteHeader(http.StatusOK)
	}))

	call := func(ip string) int {
		req := httptest.NewRequest(http.MethodPost, "/api/admin/v1/auth/login/totp", nil)
		req.Header.Set("X-Forwarded-For", ip)

		rec := httptest.NewRecorder()
		handler.ServeHTTP(rec, req)
		return rec.Code
	}

	const attacker = "203.0.113.9"
	for i := range service.RuleAdminTOTPByIP.Limit {
		if got := call(attacker); got != http.StatusOK {
			t.Fatalf("配额内第 %d 次请求应放行, got %d", i+1, got)
		}
	}
	if got := call(attacker); got != http.StatusTooManyRequests {
		t.Errorf("超出配额应返回 429, got %d", got)
	}
	if served != service.RuleAdminTOTPByIP.Limit {
		t.Errorf("被拒的请求不应到达 handler: 实际处理 %d 次, 配额 %d",
			served, service.RuleAdminTOTPByIP.Limit)
	}
}
