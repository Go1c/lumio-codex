package test

import (
	"net/http"
	"net/url"
	"testing"
	"time"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/service"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/testsupport"
)

func authorizeQuery(pkce testsupport.PKCE) string {
	return url.Values{
		"client_id":             {"cchaven-desktop"},
		"redirect_uri":          {testsupport.DesktopRedirectURI},
		"scope":                 {"profile workspace offline_access"},
		"code_challenge":        {pkce.Challenge},
		"code_challenge_method": {"S256"},
		"state":                 {"xyz"},
	}.Encode()
}

// TestAuthorizeContextForAnonymousVisitor 验证未登录也能拿到确认页所需信息，
// 前端据此先展示「谁在请求什么权限」再引导登录（交互设计 5.1）。
func TestAuthorizeContextForAnonymousVisitor(t *testing.T) {
	env := testsupport.New(t)
	pkce := testsupport.NewPKCE("anon")

	resp := env.NewClient().
		Get("/api/v1/oauth/authorize/context?" + authorizeQuery(pkce)).
		ExpectStatus(http.StatusOK)

	if resp.Data()["logged_in"] != false {
		t.Error("未登录时 logged_in 应为 false")
	}
	if got := resp.String("client_name"); got != "CC避风港 macOS" {
		t.Errorf("客户端名称 = %q", got)
	}
	if got := resp.String("redirect_kind"); got != "loopback" {
		t.Errorf("回调类型 = %q, want loopback", got)
	}
	if len(resp.Array("scopes")) != 3 {
		t.Errorf("应展示 3 项授权说明, got %v", resp.Array("scopes"))
	}
}

func TestAuthorizeContextForLoggedInUser(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")

	resp := browser.Get("/api/v1/oauth/authorize/context?" + authorizeQuery(testsupport.NewPKCE("a"))).
		ExpectStatus(http.StatusOK)

	if resp.Data()["logged_in"] != true {
		t.Error("已登录时 logged_in 应为 true")
	}
	if got := resp.String("email"); got != "alice@example.com" {
		t.Errorf("确认页应显示当前账号邮箱, got %q", got)
	}
}

// TestAuthorizeRejectsUnregisteredRedirectURI 是防开放重定向的关键用例。
func TestAuthorizeRejectsUnregisteredRedirectURI(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")
	pkce := testsupport.NewPKCE("evil")

	query := url.Values{
		"client_id":             {"cchaven-desktop"},
		"redirect_uri":          {"https://evil.example.com/callback"},
		"scope":                 {"profile"},
		"code_challenge":        {pkce.Challenge},
		"code_challenge_method": {"S256"},
	}.Encode()

	resp := browser.Post("/api/v1/oauth/authorize?"+query, nil).ExpectStatus(http.StatusBadRequest)
	if resp.ErrorCode() != "invalid_request" {
		t.Errorf("错误码 = %q, want invalid_request", resp.ErrorCode())
	}
}

// TestAuthorizeRequiresS256 验证 PKCE 只接受 S256，不允许降级为 plain。
func TestAuthorizeRequiresS256(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")

	query := url.Values{
		"client_id":             {"cchaven-desktop"},
		"redirect_uri":          {testsupport.DesktopRedirectURI},
		"scope":                 {"profile"},
		"code_challenge":        {testsupport.NewPKCE("plain").Verifier},
		"code_challenge_method": {"plain"},
	}.Encode()

	browser.Post("/api/v1/oauth/authorize?"+query, nil).ExpectStatus(http.StatusBadRequest)
}

func TestAuthorizeRequiresLogin(t *testing.T) {
	env := testsupport.New(t)

	env.NewClient().
		Post("/api/v1/oauth/authorize?"+authorizeQuery(testsupport.NewPKCE("anon")), nil).
		ExpectStatus(http.StatusUnauthorized)
}

// TestAuthorizeOnlyAcceptsAccountCentreTokens 锁住授权端点的身份来源。
//
// 本服务仍为桌面端签发会话，但「现在坐在浏览器前的是谁」只能由 Sub2API 回答。
// 拿本服务自己签的 access token 来授权新设备等于自我背书，必须拒绝——
// 否则一个被偷走的 APP 令牌就能无限繁殖出新的授权设备。
func TestAuthorizeOnlyAcceptsAccountCentreTokens(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")
	session := env.AuthorizeApp(browser, "device-1")

	env.NewClient().WithBearer(session.AccessToken).
		Post("/api/v1/oauth/authorize?"+authorizeQuery(testsupport.NewPKCE("self")), nil).
		ExpectStatus(http.StatusUnauthorized)
}

// TestAuthorizeFailsClosedWhenTheAccountCentreIsDown 账号中心不可用时不得放行授权。
func TestAuthorizeFailsClosedWhenTheAccountCentreIsDown(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")

	env.Sub2API.SetUnavailable(true)

	resp := browser.Post("/api/v1/oauth/authorize?"+authorizeQuery(testsupport.NewPKCE("down")), nil).
		ExpectStatus(http.StatusServiceUnavailable)
	if resp.ErrorCode() != "identity_unavailable" {
		t.Errorf("错误码 = %q, want identity_unavailable", resp.ErrorCode())
	}
}

// TestApproveReturnsCodeAndRedirect 验证授权码同时用于自动跳转与手动粘贴兜底。
func TestApproveReturnsCodeAndRedirect(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")
	pkce := testsupport.NewPKCE("desktop")

	resp := browser.Post("/api/v1/oauth/authorize?"+authorizeQuery(pkce), nil).
		ExpectStatus(http.StatusOK)

	code := resp.String("code")
	if code == "" {
		t.Fatal("应返回授权码供手动粘贴兜底使用")
	}

	redirect, err := url.Parse(resp.String("redirect_to"))
	if err != nil {
		t.Fatalf("回调地址解析失败: %v", err)
	}
	if redirect.Query().Get("code") != code {
		t.Error("回调地址中的 code 应与返回值一致")
	}
	if redirect.Query().Get("state") != "xyz" {
		t.Errorf("state 应原样回传, got %q", redirect.Query().Get("state"))
	}
}

// TestTokenExchangeRejectsWrongVerifier 验证 PKCE 校验真正生效：
// 截获授权码但没有 verifier 的攻击者无法换取令牌。
func TestTokenExchangeRejectsWrongVerifier(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")
	pkce := testsupport.NewPKCE("real")

	approve := browser.Post("/api/v1/oauth/authorize?"+authorizeQuery(pkce), nil).
		ExpectStatus(http.StatusOK)

	resp := env.NewClient().Post("/api/v1/oauth/token", map[string]string{
		"grant_type":    "authorization_code",
		"code":          approve.String("code"),
		"code_verifier": testsupport.NewPKCE("attacker").Verifier,
		"client_id":     "cchaven-desktop",
		"redirect_uri":  testsupport.DesktopRedirectURI,
	}).ExpectStatus(http.StatusBadRequest)

	if resp.ErrorCode() != "invalid_grant" {
		t.Errorf("错误码 = %q, want invalid_grant", resp.ErrorCode())
	}
}

// TestAuthorizationCodeIsSingleUse 验证授权码不可重放。
func TestAuthorizationCodeIsSingleUse(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")
	pkce := testsupport.NewPKCE("once")

	approve := browser.Post("/api/v1/oauth/authorize?"+authorizeQuery(pkce), nil).
		ExpectStatus(http.StatusOK)

	exchange := func() *testsupport.Response {
		return env.NewClient().Post("/api/v1/oauth/token", map[string]string{
			"grant_type":    "authorization_code",
			"code":          approve.String("code"),
			"code_verifier": pkce.Verifier,
			"client_id":     "cchaven-desktop",
			"redirect_uri":  testsupport.DesktopRedirectURI,
		})
	}

	exchange().ExpectStatus(http.StatusOK)
	exchange().ExpectStatus(http.StatusBadRequest)
}

func TestAuthorizationCodeExpires(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")
	pkce := testsupport.NewPKCE("expiring")

	approve := browser.Post("/api/v1/oauth/authorize?"+authorizeQuery(pkce), nil).
		ExpectStatus(http.StatusOK)

	env.Advance(service.AuthorizationCodeTTL + time.Minute)

	env.NewClient().Post("/api/v1/oauth/token", map[string]string{
		"grant_type":    "authorization_code",
		"code":          approve.String("code"),
		"code_verifier": pkce.Verifier,
		"client_id":     "cchaven-desktop",
		"redirect_uri":  testsupport.DesktopRedirectURI,
	}).ExpectStatus(http.StatusBadRequest)
}

// TestRefreshTokenRotation 验证轮换：旧令牌立即失效，新令牌可用。
func TestRefreshTokenRotation(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")
	session := env.AuthorizeApp(browser, "device-1")

	app := env.NewClient()
	rotated := app.Post("/api/v1/oauth/token", map[string]string{
		"grant_type": "refresh_token", "refresh_token": session.RefreshToken,
	}).ExpectStatus(http.StatusOK)

	next := rotated.String("refresh_token")
	if next == session.RefreshToken {
		t.Error("轮换后应换发新的 refresh token")
	}

	// 新令牌可继续轮换。
	app.Post("/api/v1/oauth/token", map[string]string{
		"grant_type": "refresh_token", "refresh_token": next,
	}).ExpectStatus(http.StatusOK)
}

// TestRefreshTokenReuseRevokesFamily 验证重放检测：
// 已轮换过的 refresh token 被再次出示，说明令牌外泄，整个会话族立即撤销。
func TestRefreshTokenReuseRevokesFamily(t *testing.T) {
	env := testsupport.New(t)
	browser, userID := env.SignUp("alice@example.com")
	session := env.AuthorizeApp(browser, "device-1")

	app := env.NewClient()
	rotated := app.Post("/api/v1/oauth/token", map[string]string{
		"grant_type": "refresh_token", "refresh_token": session.RefreshToken,
	}).ExpectStatus(http.StatusOK)
	freshToken := rotated.String("refresh_token")

	// 攻击者拿着已被轮换的旧令牌再次兑换。
	resp := app.Post("/api/v1/oauth/token", map[string]string{
		"grant_type": "refresh_token", "refresh_token": session.RefreshToken,
	}).ExpectStatus(http.StatusUnauthorized)
	if got := resp.ErrorMessage(); got != "登录已过期，请重新登录。" {
		t.Errorf("文案 = %q", got)
	}

	// 真实用户手里的新令牌也一并失效——这是重放处置的预期代价。
	app.Post("/api/v1/oauth/token", map[string]string{
		"grant_type": "refresh_token", "refresh_token": freshToken,
	}).ExpectStatus(http.StatusUnauthorized)

	var reason string
	if err := env.Pool.QueryRow(t.Context(),
		`SELECT revoked_reason FROM session_families WHERE user_id = $1 AND client = 'app'`,
		userID).Scan(&reason); err != nil {
		t.Fatalf("查询会话族失败: %v", err)
	}
	if reason != "reuse_detected" {
		t.Errorf("撤销原因 = %q, want reuse_detected", reason)
	}
}

// TestAppSessionAppearsInDeviceList 验证经浏览器授权的 APP 出现在「登录设备与授权」列表里。
//
// 列表里只有 APP 会话：官网侧拿的是 Sub2API 令牌，本服务不再为浏览器建会话族。
func TestAppSessionAppearsInDeviceList(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")
	session := env.AuthorizeApp(browser, "device-1")

	items := browser.Get("/api/v1/me/sessions").ExpectStatus(http.StatusOK).Array("items")
	if len(items) != 1 {
		t.Fatalf("应只有 APP 一个会话, got %d", len(items))
	}

	item := items[0].(map[string]any)
	if item["kind"] != "app" {
		t.Errorf("会话类型 = %v, want app", item["kind"])
	}
	if got := item["device_name"]; got != "MacBook Pro — CC避风港 APP 1.4.2" {
		t.Errorf("设备名 = %v", got)
	}
	if got := item["platform_detail"]; got != "macOS 15 · Apple Silicon" {
		t.Errorf("平台信息 = %v", got)
	}
	// 门户不是本地会话，因此在它眼里没有「本设备」。
	if item["current"] == true {
		t.Error("从账号中心看列表时不应把 APP 标成本设备")
	}

	// APP 自己看列表时，才认得出哪一条是自己。
	fromApp := env.NewClient().WithBearer(session.AccessToken).
		Get("/api/v1/me/sessions").ExpectStatus(http.StatusOK).Array("items")
	if fromApp[0].(map[string]any)["current"] != true {
		t.Error("APP 应把自己的会话标记为本设备")
	}
}

// TestRevokeSessionLogsOutThatDevice 验证「在这里退出即可撤销该设备的授权」。
func TestRevokeSessionLogsOutThatDevice(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")
	session := env.AuthorizeApp(browser, "device-1")

	appClient := env.NewClient().WithBearer(session.AccessToken)
	appClient.Get("/api/v1/me").ExpectStatus(http.StatusOK)

	var appSessionID string
	for _, raw := range browser.Get("/api/v1/me/sessions").Array("items") {
		if item := raw.(map[string]any); item["kind"] == "app" {
			appSessionID = item["id"].(string)
		}
	}

	browser.Delete("/api/v1/me/sessions/" + appSessionID).ExpectStatus(http.StatusNoContent)

	// access token 尚未过期，但每次请求都会回查会话族，因此立即失效。
	appClient.Get("/api/v1/me").ExpectStatus(http.StatusUnauthorized)
	browser.Get("/api/v1/me").ExpectStatus(http.StatusOK)
}

// TestRevokeOtherSessionsKeepsCurrent 验证「退出所有其他设备」保留当前会话。
func TestRevokeOtherSessionsKeepsCurrent(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")
	first := env.AuthorizeApp(browser, "device-1")
	second := env.AuthorizeApp(browser, "device-2")

	app := env.NewClient().WithBearer(first.AccessToken)
	resp := app.Post("/api/v1/me/sessions/revoke-others", nil).ExpectStatus(http.StatusOK)
	if got := resp.Number("revoked"); got != 1 {
		t.Errorf("应撤销 1 个其他会话, got %v", got)
	}

	app.Get("/api/v1/me").ExpectStatus(http.StatusOK)
	env.NewClient().WithBearer(second.AccessToken).Get("/api/v1/me").ExpectStatus(http.StatusUnauthorized)
}

// TestPortalRevokeOtherSessionsClearsEveryDevice 从账号中心发起时没有「本设备」，
// 因此所有 APP 会话都会被清掉。
func TestPortalRevokeOtherSessionsClearsEveryDevice(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")
	first := env.AuthorizeApp(browser, "device-1")
	second := env.AuthorizeApp(browser, "device-2")

	resp := browser.Post("/api/v1/me/sessions/revoke-others", nil).ExpectStatus(http.StatusOK)
	if got := resp.Number("revoked"); got != 2 {
		t.Errorf("应撤销 2 个会话, got %v", got)
	}

	browser.Get("/api/v1/me").ExpectStatus(http.StatusOK)
	env.NewClient().WithBearer(first.AccessToken).Get("/api/v1/me").ExpectStatus(http.StatusUnauthorized)
	env.NewClient().WithBearer(second.AccessToken).Get("/api/v1/me").ExpectStatus(http.StatusUnauthorized)
}
