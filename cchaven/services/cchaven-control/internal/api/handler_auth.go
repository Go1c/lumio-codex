package api

import (
	"net/http"

	"github.com/go-chi/chi/v5"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/apperr"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/httpx"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/i18n"
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

type registerRequest struct {
	Email     string `json:"email"`
	Password  string `json:"password"`
	UTMSource string `json:"utm_source,omitempty"`
}

func (s *Server) handleRegister(w http.ResponseWriter, r *http.Request) {
	var req registerRequest
	if err := httpx.DecodeJSON(w, r, &req); err != nil {
		httpx.Fail(w, r, err)
		return
	}

	visitorID := s.referralVisitorFrom(r)
	result, err := s.svc.Register(r.Context(), service.RegisterInput{
		Email:        req.Email,
		Password:     req.Password,
		ReferralCode: s.referralCodeFrom(r),
		VisitorID:    &visitorID,
		UTMSource:    req.UTMSource,
		IP:           httpx.ClientIP(r),
		UserAgent:    httpx.UserAgent(r),
	})
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusCreated, result)
}

type verifyEmailRequest struct {
	Email string `json:"email"`
	Code  string `json:"code"`
}

func (s *Server) handleVerifyEmail(w http.ResponseWriter, r *http.Request) {
	var req verifyEmailRequest
	if err := httpx.DecodeJSON(w, r, &req); err != nil {
		httpx.Fail(w, r, err)
		return
	}

	user, pair, err := s.svc.VerifyEmail(r.Context(), service.VerifyEmailInput{
		Email:     req.Email,
		Code:      req.Code,
		IP:        httpx.ClientIP(r),
		UserAgent: httpx.UserAgent(r),
	})
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}

	entitlement, err := s.svc.Entitlement(r.Context(), user.ID)
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}

	s.setSessionCookies(w, pair)
	httpx.JSON(w, http.StatusOK, map[string]any{
		"user":        service.ViewUser(user),
		"entitlement": entitlement,
	})
}

type emailOnlyRequest struct {
	Email string `json:"email"`
}

func (s *Server) handleResendCode(w http.ResponseWriter, r *http.Request) {
	var req emailOnlyRequest
	if err := httpx.DecodeJSON(w, r, &req); err != nil {
		httpx.Fail(w, r, err)
		return
	}

	cooldown, devCode, err := s.svc.ResendVerificationCode(r.Context(), req.Email, httpx.ClientIP(r))
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}

	body := map[string]any{"retry_after_seconds": cooldown}
	if devCode != "" {
		body["dev_code"] = devCode
	}
	httpx.JSON(w, http.StatusAccepted, body)
}

type loginRequest struct {
	Email    string `json:"email"`
	Password string `json:"password"`
}

func (s *Server) handleLogin(w http.ResponseWriter, r *http.Request) {
	var req loginRequest
	if err := httpx.DecodeJSON(w, r, &req); err != nil {
		httpx.Fail(w, r, err)
		return
	}

	user, pair, err := s.svc.Login(r.Context(), service.LoginInput{
		Email:     req.Email,
		Password:  req.Password,
		IP:        httpx.ClientIP(r),
		UserAgent: httpx.UserAgent(r),
	})
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}

	entitlement, err := s.svc.Entitlement(r.Context(), user.ID)
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}

	s.setSessionCookies(w, pair)
	httpx.JSON(w, http.StatusOK, map[string]any{
		"user":        service.ViewUser(user),
		"entitlement": entitlement,
	})
}

func (s *Server) handleForgotPassword(w http.ResponseWriter, r *http.Request) {
	var req emailOnlyRequest
	if err := httpx.DecodeJSON(w, r, &req); err != nil {
		httpx.Fail(w, r, err)
		return
	}

	devToken, err := s.svc.RequestPasswordReset(r.Context(), req.Email, httpx.ClientIP(r))
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}

	// 无论邮箱是否注册都返回同一句话，这是 6.2 节规定的防枚举回执。
	body := map[string]any{
		"message": i18n.T(httpx.LangOf(r), i18n.MsgForgotPasswordSubmitted,
			map[string]string{"email": req.Email}),
	}
	if devToken != "" {
		body["dev_token"] = devToken
	}
	httpx.JSON(w, http.StatusAccepted, body)
}

func (s *Server) handleInspectResetToken(w http.ResponseWriter, r *http.Request) {
	masked, err := s.svc.InspectPasswordResetToken(r.Context(), chi.URLParam(r, "token"))
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, map[string]any{"valid": true, "email_masked": masked})
}

type resetPasswordRequest struct {
	Token    string `json:"token"`
	Password string `json:"password"`
}

func (s *Server) handleResetPassword(w http.ResponseWriter, r *http.Request) {
	var req resetPasswordRequest
	if err := httpx.DecodeJSON(w, r, &req); err != nil {
		httpx.Fail(w, r, err)
		return
	}

	if err := s.svc.ResetPassword(r.Context(), req.Token, req.Password); err != nil {
		httpx.Fail(w, r, err)
		return
	}

	s.clearSessionCookies(w)
	httpx.JSON(w, http.StatusOK, map[string]any{
		"message": i18n.T(httpx.LangOf(r), i18n.MsgPasswordUpdatedAll, nil),
	})
}

type refreshRequest struct {
	RefreshToken string `json:"refresh_token,omitempty"`
}

func (s *Server) handleRefresh(w http.ResponseWriter, r *http.Request) {
	req := refreshRequest{}
	if r.ContentLength > 0 {
		if err := httpx.DecodeJSON(w, r, &req); err != nil {
			httpx.Fail(w, r, err)
			return
		}
	}
	if req.RefreshToken == "" {
		if cookie, err := r.Cookie(s.cfg.CookieName.Refresh); err == nil {
			req.RefreshToken = cookie.Value
		}
	}

	pair, err := s.svc.RefreshSession(r.Context(), req.RefreshToken)
	if err != nil {
		s.clearSessionCookies(w)
		httpx.Fail(w, r, err)
		return
	}

	s.setSessionCookies(w, pair)
	httpx.JSON(w, http.StatusOK, map[string]any{"expires_in": pair.ExpiresIn})
}

func (s *Server) handleLogout(w http.ResponseWriter, r *http.Request) {
	// 即便令牌已失效也照常清 cookie 并返回成功，避免用户卡在「登不出去」的状态。
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
