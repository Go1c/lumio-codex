package api

import (
	"net/http"

	"github.com/go-chi/chi/v5"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/apperr"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/httpx"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/service"
)

func (s *Server) handleHealth(w http.ResponseWriter, _ *http.Request) {
	httpx.JSON(w, http.StatusOK, map[string]string{"status": "ok"})
}

func (s *Server) handlePublicConfig(w http.ResponseWriter, r *http.Request) {
	cfg, err := s.svc.PublicConfig(r.Context())
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, cfg)
}

// handleLogout 清理本地会话。
//
// 身份虽然归 Sub2API，本服务仍然为桌面端签发自己的会话族，退出登录要把它撤掉。
// 即便令牌已失效也照常清 cookie 并返回成功，避免用户卡在「登不出去」的状态。
func (s *Server) handleLogout(w http.ResponseWriter, r *http.Request) {
	if token, _ := s.accessTokenFrom(r); token != "" {
		if principal, err := s.svc.AuthenticateAccess(r.Context(), token); err == nil {
			if err := s.svc.Logout(r.Context(), principal.SessionID); err != nil {
				httpx.Fail(w, r, err)
				return
			}
		}
	}

	s.clearSessionCookies(w)
	httpx.NoContent(w)
}

func (s *Server) handleSession(w http.ResponseWriter, r *http.Request) {
	principal := principalOf(r)

	entitlement, err := s.svc.Entitlement(r.Context(), principal.User.ID)
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, map[string]any{
		"user":        service.ViewUser(principal.User),
		"entitlement": entitlement,
	})
}

func (s *Server) handleInviteLanding(w http.ResponseWriter, r *http.Request) {
	code := chi.URLParam(r, "code")
	if code == "" {
		httpx.Fail(w, r, apperr.InvalidParams())
		return
	}

	visitorID := s.referralVisitorFrom(r)
	landing, err := s.svc.ResolveInvite(r.Context(), code, visitorID, httpx.ClientIP(r), httpx.UserAgent(r))
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}

	// 只有有效邀请码才写 cookie，避免失效码污染后续注册归因。
	if landing.Valid {
		s.setReferralCookies(w, code, visitorID)
	}
	httpx.JSON(w, http.StatusOK, landing)
}

// handleCurrentInvite 回答「当前浏览器还带着有效邀请吗」，供首页邀请横幅决定是否高亮。
//
// cch_ref 是 HttpOnly 的，前端读不到，所以横幅必须由服务端给出权威答案；
// 只读不写，既不下发 cookie 也不记访问。
func (s *Server) handleCurrentInvite(w http.ResponseWriter, r *http.Request) {
	attribution, err := s.svc.CurrentInvite(r.Context(), s.referralCodeFrom(r))
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, attribution)
}
