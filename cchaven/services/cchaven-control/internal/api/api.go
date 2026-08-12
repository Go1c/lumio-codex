// Package api 是 HTTP 传输层：路由、中间件与请求/响应编解码。
//
// 这一层只做协议转换，所有业务规则都在 internal/service。
package api

import (
	"net/http"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/go-chi/chi/v5/middleware"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/config"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/service"
)

// Server 承载路由与依赖。
type Server struct {
	svc *service.Service
	cfg config.Config
}

// NewServer 构造 HTTP 服务。
func NewServer(svc *service.Service, cfg config.Config) *Server {
	return &Server{svc: svc, cfg: cfg}
}

// Routes 组装完整路由表。
func (s *Server) Routes() http.Handler {
	r := chi.NewRouter()

	r.Use(middleware.RequestID)
	r.Use(middleware.RealIP)
	r.Use(middleware.Recoverer)
	r.Use(middleware.Timeout(30 * time.Second))
	r.Use(s.cors)

	r.Route("/api/v1", func(r chi.Router) {
		// 健康检查不限频，避免探针被配额挡住而误判服务不可用。
		r.Get("/health", s.handleHealth)
		r.With(s.rateLimitPublic).Get("/config/public", s.handlePublicConfig)

		r.Route("/auth", func(r chi.Router) {
			r.Post("/register", s.handleRegister)
			r.Post("/verify-email", s.handleVerifyEmail)
			r.Post("/verification-code/resend", s.handleResendCode)
			r.Post("/login", s.handleLogin)
			r.Post("/password/forgot", s.handleForgotPassword)
			r.Get("/password/reset/{token}", s.handleInspectResetToken)
			r.Post("/password/reset", s.handleResetPassword)
			r.Post("/refresh", s.handleRefresh)
			r.Post("/logout", s.handleLogout)

			r.With(s.requireUser).Get("/session", s.handleSession)
		})

		r.Route("/oauth", func(r chi.Router) {
			r.Get("/authorize/context", s.handleAuthorizeContext)
			r.With(s.requireUser).Post("/authorize", s.handleAuthorizeApprove)
			r.Post("/token", s.handleToken)
			r.Post("/revoke", s.handleRevokeToken)
		})

		// current 是静态段，chi 的路由树优先于 {code} 匹配；且邀请码固定 8 位，
		// 与 7 个字符的 current 不可能撞车，两条路由可以安全共存。
		r.With(s.rateLimitPublic).Get("/invites/current", s.handleCurrentInvite)
		r.With(s.rateLimitPublic).Get("/invites/{code}", s.handleInviteLanding)

		r.Route("/me", func(r chi.Router) {
			r.Use(s.requireUser)
			r.Get("/", s.handleMe)
			r.Patch("/", s.handleUpdateProfile)
			r.Get("/entitlement", s.handleEntitlement)
			r.Post("/password", s.handleChangePassword)
			r.Post("/email-change", s.handleRequestEmailChange)
			r.Post("/email-change/verify", s.handleConfirmEmailChange)
			r.Delete("/email-change", s.handleCancelEmailChange)
			r.Get("/sessions", s.handleListSessions)
			r.Delete("/sessions/{id}", s.handleRevokeSession)
			r.Post("/sessions/revoke-others", s.handleRevokeOtherSessions)
			r.Get("/referrals", s.handleReferrals)
			r.Post("/deletion", s.handleRequestDeletion)
			r.Delete("/deletion", s.handleCancelDeletion)
		})

		r.Route("/billing", func(r chi.Router) {
			r.Get("/plan", s.handlePlan)
			r.Post("/webhook/{provider}", s.handleWebhook)

			r.Group(func(r chi.Router) {
				r.Use(s.requireUser)
				r.Post("/checkout", s.handleCheckout)
				r.Get("/orders", s.handleListMyOrders)
				r.Get("/orders/{orderNo}", s.handleGetMyOrder)
			})
		})

		r.With(s.requireUser).Post("/app/heartbeat", s.handleHeartbeat)
	})

	// 管理端。requireAdmin 只负责「是不是通过两步验证的管理员」，不区分角色；
	// 角色由 service 层的能力谓词裁决（矩阵见 internal/service/admin.go 的 roleCapabilities）。
	// 新增写操作或敏感读取时，除了在这里挂路由，还要在对应 service 方法入口调一次 Can*，
	// 拒绝路径走 auditDenied 写 `{action}_denied` 再返回 403。
	r.Route("/api/admin/v1", func(r chi.Router) {
		r.Post("/auth/login", s.handleAdminLogin)
		r.Post("/auth/login/totp", s.handleAdminTOTP)

		r.Group(func(r chi.Router) {
			r.Use(s.requireAdmin)
			r.Post("/auth/logout", s.handleAdminLogout)
			r.Get("/auth/me", s.handleAdminMe)
			r.Post("/auth/totp/setup", s.handleAdminTOTPSetup)
			r.Post("/auth/totp/enable", s.handleAdminTOTPEnable)

			r.Get("/metrics/overview", s.handleMetricsOverview)
			r.Get("/metrics/dau", s.handleMetricsDAU)
			r.Get("/metrics/distributions", s.handleMetricsDistributions)

			r.Get("/users", s.handleAdminListUsers)
			r.Get("/users/{id}", s.handleAdminGetUser)
			r.Post("/users/{id}/disable", s.handleAdminDisableUser)
			r.Post("/users/{id}/enable", s.handleAdminEnableUser)

			r.Get("/orders", s.handleAdminListOrders)
			r.Get("/orders/export", s.handleAdminExportOrders)
			r.Post("/orders/{orderNo}/refund", s.handleAdminRefund)

			r.Get("/configs", s.handleAdminGetConfigs)
			r.Put("/configs", s.handleAdminPutConfigs)

			r.Get("/audit-logs", s.handleAdminAuditLogs)
		})
	})

	return r
}

// cors 允许官网与管理后台跨源携带 cookie 访问。
func (s *Server) cors(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// 响应内容随 Origin 而变，无论最终放不放行都要声明，
		// 否则共享缓存可能把官网的响应连同它的 Allow-Origin 头喂给后台。
		w.Header().Add("Vary", "Origin")

		origin := r.Header.Get("Origin")
		if s.allowedOrigin(origin) {
			w.Header().Set("Access-Control-Allow-Origin", origin)
			w.Header().Set("Access-Control-Allow-Credentials", "true")
		}

		if r.Method == http.MethodOptions {
			w.Header().Set("Access-Control-Allow-Methods", "GET,POST,PATCH,DELETE,PUT,OPTIONS")
			w.Header().Set("Access-Control-Allow-Headers", "Content-Type,Authorization,X-CCHaven-Signature")
			w.Header().Set("Access-Control-Max-Age", "600")
			w.WriteHeader(http.StatusNoContent)
			return
		}
		next.ServeHTTP(w, r)
	})
}
