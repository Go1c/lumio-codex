package service

import (
	"context"
	"errors"
	"net/url"
	"strings"

	"github.com/jackc/pgx/v5"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/apperr"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/db"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/domain"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/security"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/store"
)

// scopeLabels 是授权确认页展示给用户的权限说明。
var scopeLabels = map[string]string{
	"profile":        "读取你的账号邮箱与订阅状态",
	"workspace":      "代表你连接与同步你的工作区",
	"offline_access": "在你未打开浏览器时保持登录",
}

// AuthorizeRequest 是 /authorize 的查询参数。
type AuthorizeRequest struct {
	ClientID            string
	RedirectURI         string
	Scope               string
	CodeChallenge       string
	CodeChallengeMethod string
	State               string
}

// ScopeItem 是确认页上的一项授权说明。
type ScopeItem struct {
	ID    string `json:"id"`
	Label string `json:"label"`
}

// AuthorizeContext 供 /authorize 确认页渲染。
type AuthorizeContext struct {
	ClientName   string      `json:"client_name"`
	Scopes       []ScopeItem `json:"scopes"`
	RedirectKind string      `json:"redirect_kind"` // loopback | scheme
	LoggedIn     bool        `json:"logged_in"`
	Email        string      `json:"email,omitempty"`
}

// validateAuthorizeRequest 校验授权请求参数。
//
// PKCE 强制 S256，redirect_uri 必须匹配客户端注册的模式，
// 二者共同防止授权码被本机其他程序或恶意重定向截获。
func (s *Service) validateAuthorizeRequest(
	ctx context.Context, q store.Querier, req AuthorizeRequest,
) (store.OAuthClient, error) {
	client, err := store.GetOAuthClient(ctx, q, req.ClientID)
	if err != nil {
		if errors.Is(err, store.ErrNotFound) {
			return store.OAuthClient{}, apperr.OAuthInvalidRequest("未知的 client_id")
		}
		return store.OAuthClient{}, err
	}
	if !client.AllowsRedirectURI(req.RedirectURI) {
		return store.OAuthClient{}, apperr.OAuthInvalidRequest("redirect_uri 未注册")
	}
	if req.CodeChallengeMethod != "S256" {
		return store.OAuthClient{}, apperr.OAuthInvalidRequest("code_challenge_method 必须为 S256")
	}
	if len(req.CodeChallenge) < 43 {
		return store.OAuthClient{}, apperr.OAuthInvalidRequest("code_challenge 无效")
	}
	if req.Scope == "" {
		return store.OAuthClient{}, apperr.OAuthInvalidRequest("缺少 scope")
	}
	if !client.AllowsScope(req.Scope) {
		return store.OAuthClient{}, apperr.OAuthInvalidRequest("scope 超出客户端权限")
	}
	return client, nil
}

// AuthorizeContextFor 组装确认页所需数据。未登录时同样返回参数校验结果，
// 让前端能先展示「谁在请求什么权限」，再引导登录。
func (s *Service) AuthorizeContextFor(
	ctx context.Context, req AuthorizeRequest, viewer *domain.User,
) (AuthorizeContext, error) {
	client, err := s.validateAuthorizeRequest(ctx, s.Pool, req)
	if err != nil {
		return AuthorizeContext{}, err
	}

	scopes := make([]ScopeItem, 0, 3)
	for _, id := range strings.Fields(req.Scope) {
		label, ok := scopeLabels[id]
		if !ok {
			label = id
		}
		scopes = append(scopes, ScopeItem{ID: id, Label: label})
	}

	out := AuthorizeContext{
		ClientName:   client.Name,
		Scopes:       scopes,
		RedirectKind: redirectKind(req.RedirectURI),
	}
	if viewer != nil {
		out.LoggedIn = true
		out.Email = viewer.Email
	}
	return out, nil
}

// ApproveInput 是用户在确认页点「授权」时提交的数据。
type ApproveInput struct {
	Request    AuthorizeRequest
	UserID     int64
	DeviceName string
	OSVersion  string
	Arch       string
	AppVersion string
}

// ApproveResult 是授权结果。
//
// Code 除了拼进 RedirectTo，也直接回传给页面：桌面端「手动粘贴授权码」兜底
// （交互设计 5.1 超时态）依赖页面把它展示出来。
type ApproveResult struct {
	Code       string `json:"code"`
	RedirectTo string `json:"redirect_to"`
	ExpiresIn  int    `json:"expires_in"`
}

// Approve 签发授权码。
func (s *Service) Approve(ctx context.Context, in ApproveInput) (ApproveResult, error) {
	client, err := s.validateAuthorizeRequest(ctx, s.Pool, in.Request)
	if err != nil {
		return ApproveResult{}, err
	}

	code, err := security.RandomToken(AuthorizationCodeBytes)
	if err != nil {
		return ApproveResult{}, err
	}

	now := s.now()
	if err := store.CreateAuthorizationCode(ctx, s.Pool, security.HashToken(code), store.AuthorizationCode{
		ClientID:      client.ID,
		UserID:        in.UserID,
		RedirectURI:   in.Request.RedirectURI,
		Scope:         in.Request.Scope,
		CodeChallenge: in.Request.CodeChallenge,
		DeviceName:    in.DeviceName,
		Platform:      "macos",
		OSVersion:     in.OSVersion,
		Arch:          in.Arch,
		AppVersion:    in.AppVersion,
		ExpiresAt:     now.Add(AuthorizationCodeTTL),
	}); err != nil {
		return ApproveResult{}, err
	}

	return ApproveResult{
		Code:       code,
		RedirectTo: buildRedirect(in.Request.RedirectURI, code, in.Request.State),
		ExpiresIn:  int(AuthorizationCodeTTL.Seconds()),
	}, nil
}

// ExchangeInput 是授权码换令牌的请求。
type ExchangeInput struct {
	Code         string
	CodeVerifier string
	ClientID     string
	RedirectURI  string
	DeviceID     string
	IP           string
	UserAgent    string
}

// ExchangeResult 是令牌兑换结果，含首次登录 APP 时的发放结果。
type ExchangeResult struct {
	Tokens     TokenPair
	Activation ActivationResult
}

// ExchangeCode 用授权码换取令牌，并在首次 APP 登录时触发试用与邀请奖励结算。
func (s *Service) ExchangeCode(ctx context.Context, in ExchangeInput) (ExchangeResult, error) {
	now := s.now()
	var out ExchangeResult

	err := db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		// 取码与标记已用是同一条 UPDATE，并发兑换只有一方能成功。
		record, err := store.ConsumeAuthorizationCode(ctx, tx, security.HashToken(in.Code), now)
		if err != nil {
			if errors.Is(err, store.ErrNotFound) {
				return apperr.OAuthInvalidGrant()
			}
			return err
		}
		if record.ClientID != in.ClientID || record.RedirectURI != in.RedirectURI {
			return apperr.OAuthInvalidGrant()
		}
		if !security.VerifyPKCE(in.CodeVerifier, record.CodeChallenge) {
			return apperr.OAuthInvalidGrant()
		}

		user, err := store.GetUserByID(ctx, tx, record.UserID)
		if err != nil {
			return err
		}
		if user.Status == domain.UserDisabled {
			return apperr.AccountDisabled()
		}
		if user.Status != domain.UserActive {
			return apperr.OAuthInvalidGrant()
		}

		// 「首次登录 APP」必须在建立本次会话之前判定，否则本次会话会把自己算进去。
		hadAppSession, err := store.HasAppSession(ctx, tx, user.ID)
		if err != nil {
			return err
		}

		out.Tokens, err = s.issueSession(ctx, tx, store.CreateSessionFamilyParams{
			UserID:      user.ID,
			Client:      domain.ClientApp,
			OAuthClient: record.ClientID,
			DeviceName:  deviceLabel(record.DeviceName, record.AppVersion),
			Platform:    record.Platform,
			OSVersion:   record.OSVersion,
			Arch:        record.Arch,
			AppVersion:  record.AppVersion,
			UserAgent:   in.UserAgent,
			IP:          in.IP,
		}, record.Scope)
		if err != nil {
			return err
		}

		if err := store.RecordActivity(ctx, tx, user.ID, now); err != nil {
			return err
		}
		if in.DeviceID != "" {
			if err := store.UpsertDevice(ctx, tx, user.ID, in.DeviceID,
				record.Platform, record.OSVersion, record.Arch, record.AppVersion, now); err != nil {
				return err
			}
		}

		if !hadAppSession {
			out.Activation, err = s.SettleFirstAppLogin(ctx, tx, ActivationInput{
				UserID:   user.ID,
				DeviceID: in.DeviceID,
				SignupIP: in.IP,
			})
			if err != nil {
				return err
			}
		}
		return nil
	})

	return out, err
}

// RevokeToken 撤销 refresh token 所属的会话族，对应 APP 退出登录。
func (s *Service) RevokeToken(ctx context.Context, refreshToken string) error {
	record, err := store.GetRefreshTokenByHash(ctx, s.Pool, security.HashToken(refreshToken))
	if err != nil {
		// 撤销接口对未知令牌返回成功，避免变成令牌有效性探针。
		if errors.Is(err, store.ErrNotFound) {
			return nil
		}
		return err
	}

	err = store.RevokeSessionFamily(ctx, s.Pool, record.FamilyID, domain.RevokeUserLogout, s.now())
	if errors.Is(err, store.ErrNotFound) {
		return nil
	}
	return err
}

func redirectKind(uri string) string {
	if strings.HasPrefix(uri, "http://") {
		return "loopback"
	}
	return "scheme"
}

// buildRedirect 把授权码与 state 追加到回调地址。
// redirect_uri 已通过白名单校验；state 由客户端提供，必须转义后再拼接。
func buildRedirect(redirectURI, code, state string) string {
	params := url.Values{"code": {code}}
	if state != "" {
		params.Set("state", state)
	}

	separator := "?"
	if strings.Contains(redirectURI, "?") {
		separator = "&"
	}
	return redirectURI + separator + params.Encode()
}

// deviceLabel 组合设备名与版本号，形如「MacBook Pro — CC避风港 APP 1.4.2」。
func deviceLabel(deviceName, appVersion string) string {
	if deviceName == "" {
		deviceName = "macOS 设备"
	}
	if appVersion == "" {
		return deviceName
	}
	return deviceName + " — CC避风港 APP " + appVersion
}
