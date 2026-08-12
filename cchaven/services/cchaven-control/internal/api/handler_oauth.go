package api

import (
	"net/http"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/apperr"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/domain"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/httpx"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/service"
)

// authorizeRequestFrom 解析 /authorize 的查询参数。
func authorizeRequestFrom(r *http.Request) service.AuthorizeRequest {
	q := r.URL.Query()
	return service.AuthorizeRequest{
		ClientID:            q.Get("client_id"),
		RedirectURI:         q.Get("redirect_uri"),
		Scope:               q.Get("scope"),
		CodeChallenge:       q.Get("code_challenge"),
		CodeChallengeMethod: q.Get("code_challenge_method"),
		State:               q.Get("state"),
	}
}

// handleAuthorizeContext 供 /authorize 确认页渲染。
// 未登录也返回 200：页面要先告诉用户「谁在请求什么权限」，再引导登录。
func (s *Server) handleAuthorizeContext(w http.ResponseWriter, r *http.Request) {
	var viewer *domain.User
	if token, viaCookie := s.accessTokenFrom(r); token != "" && viaCookie {
		if principal, err := s.svc.AuthenticateAccess(r.Context(), token); err == nil {
			viewer = &principal.User
		}
	}

	ctx, err := s.svc.AuthorizeContextFor(r.Context(), authorizeRequestFrom(r), viewer)
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, ctx)
}

type approveRequest struct {
	DeviceName string `json:"device_name,omitempty"`
	OSVersion  string `json:"os_version,omitempty"`
	Arch       string `json:"arch,omitempty"`
	AppVersion string `json:"app_version,omitempty"`
}

// handleAuthorizeApprove 处理用户点击「授权」。
func (s *Server) handleAuthorizeApprove(w http.ResponseWriter, r *http.Request) {
	req := approveRequest{}
	if r.ContentLength > 0 {
		if err := httpx.DecodeJSON(w, r, &req); err != nil {
			httpx.Fail(w, r, err)
			return
		}
	}

	principal := principalOf(r)
	result, err := s.svc.Approve(r.Context(), service.ApproveInput{
		Request:    authorizeRequestFrom(r),
		UserID:     principal.User.ID,
		DeviceName: req.DeviceName,
		OSVersion:  req.OSVersion,
		Arch:       req.Arch,
		AppVersion: req.AppVersion,
	})
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, result)
}

type tokenRequest struct {
	GrantType    string `json:"grant_type"`
	Code         string `json:"code,omitempty"`
	CodeVerifier string `json:"code_verifier,omitempty"`
	ClientID     string `json:"client_id,omitempty"`
	RedirectURI  string `json:"redirect_uri,omitempty"`
	RefreshToken string `json:"refresh_token,omitempty"`
	DeviceID     string `json:"device_id,omitempty"`
}

type tokenResponse struct {
	AccessToken  string                    `json:"access_token"`
	RefreshToken string                    `json:"refresh_token"`
	TokenType    string                    `json:"token_type"`
	ExpiresIn    int                       `json:"expires_in"`
	Activation   *service.ActivationResult `json:"activation,omitempty"`
	Entitlement  *domain.Entitlement       `json:"entitlement,omitempty"`
}

// handleToken 兑换授权码或轮换 refresh token。
func (s *Server) handleToken(w http.ResponseWriter, r *http.Request) {
	var req tokenRequest
	if err := httpx.DecodeJSON(w, r, &req); err != nil {
		httpx.Fail(w, r, err)
		return
	}

	switch req.GrantType {
	case "authorization_code":
		s.exchangeAuthorizationCode(w, r, req)
	case "refresh_token":
		s.exchangeRefreshToken(w, r, req)
	default:
		httpx.Fail(w, r, apperr.OAuthInvalidRequest("不支持的 grant_type"))
	}
}

func (s *Server) exchangeAuthorizationCode(w http.ResponseWriter, r *http.Request, req tokenRequest) {
	result, err := s.svc.ExchangeCode(r.Context(), service.ExchangeInput{
		Code:         req.Code,
		CodeVerifier: req.CodeVerifier,
		ClientID:     req.ClientID,
		RedirectURI:  req.RedirectURI,
		DeviceID:     req.DeviceID,
		IP:           httpx.ClientIP(r),
		UserAgent:    httpx.UserAgent(r),
	})
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}

	principal, err := s.svc.AuthenticateAccess(r.Context(), result.Tokens.AccessToken)
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	entitlement, err := s.svc.Entitlement(r.Context(), principal.User.ID)
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}

	httpx.JSON(w, http.StatusOK, tokenResponse{
		AccessToken:  result.Tokens.AccessToken,
		RefreshToken: result.Tokens.RefreshToken,
		TokenType:    "Bearer",
		ExpiresIn:    result.Tokens.ExpiresIn,
		Activation:   &result.Activation,
		Entitlement:  &entitlement,
	})
}

func (s *Server) exchangeRefreshToken(w http.ResponseWriter, r *http.Request, req tokenRequest) {
	pair, err := s.svc.RefreshSession(r.Context(), req.RefreshToken)
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, tokenResponse{
		AccessToken:  pair.AccessToken,
		RefreshToken: pair.RefreshToken,
		TokenType:    "Bearer",
		ExpiresIn:    pair.ExpiresIn,
	})
}

type revokeRequest struct {
	Token string `json:"token"`
}

func (s *Server) handleRevokeToken(w http.ResponseWriter, r *http.Request) {
	var req revokeRequest
	if err := httpx.DecodeJSON(w, r, &req); err != nil {
		httpx.Fail(w, r, err)
		return
	}
	if err := s.svc.RevokeToken(r.Context(), req.Token); err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.NoContent(w)
}
