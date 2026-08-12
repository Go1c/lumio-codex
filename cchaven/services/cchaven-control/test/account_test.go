package test

import (
	"net/http"
	"testing"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/testsupport"
)

func TestMeReturnsDisplayIDAndEntitlement(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")

	resp := browser.Get("/api/v1/me").ExpectStatus(http.StatusOK)

	user := resp.Object("user")
	// 对外展示的注册号形如 U-100986，序列自 100000 起。
	if got := user["id"]; got != "U-100000" {
		t.Errorf("注册号 = %v, want U-100000", got)
	}
	if got := user["email"]; got != "alice@example.com" {
		t.Errorf("邮箱 = %v", got)
	}
	if got := resp.Object("entitlement")["status"]; got != "none" {
		t.Errorf("订阅状态 = %v, want none", got)
	}
}

func TestUpdateDisplayName(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")

	resp := browser.Patch("/api/v1/me", map[string]string{"display_name": "Mary"}).
		ExpectStatus(http.StatusOK)

	if got := resp.String("display_name"); got != "Mary" {
		t.Errorf("显示名称 = %q", got)
	}
	if got := browser.Get("/api/v1/me").Object("user")["display_name"]; got != "Mary" {
		t.Errorf("重新读取的显示名称 = %v", got)
	}
}

// TestAccountDeletionHasSevenDayGracePeriod 验证注销 7 天冷静期与撤销。
//
// 注销的是 CC 侧的影子账号与业务数据；Sub2API 那边的账号由账号中心自己管。
func TestAccountDeletionHasSevenDayGracePeriod(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")

	resp := browser.Post("/api/v1/me/deletion", nil).ExpectStatus(http.StatusOK)
	if resp.String("effective_at") == "" {
		t.Error("应返回注销生效时间")
	}

	// 冷静期内账号仍可用。
	browser.Get("/api/v1/me").ExpectStatus(http.StatusOK)
	if got := browser.Get("/api/v1/me").Object("user")["deletion_requested_at"]; got == nil {
		t.Error("应回传注销申请时间供前端展示可撤销窗口")
	}

	browser.Delete("/api/v1/me/deletion").ExpectStatus(http.StatusNoContent)
	if got := browser.Get("/api/v1/me").Object("user")["deletion_requested_at"]; got != nil {
		t.Errorf("撤销后不应再有注销申请, got %v", got)
	}
}

// TestLogoutRevokesTheAppSession 验证退出登录撤销本服务签发的会话。
//
// 身份归账号中心，但桌面端的会话族仍由本服务签发，退出时必须真的作废。
func TestLogoutRevokesTheAppSession(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")
	session := env.AuthorizeApp(browser, "device-1")

	app := env.NewClient().WithBearer(session.AccessToken)
	app.Get("/api/v1/me").ExpectStatus(http.StatusOK)

	app.Post("/api/v1/auth/logout", nil).ExpectStatus(http.StatusNoContent)
	app.Get("/api/v1/me").ExpectStatus(http.StatusUnauthorized)
}

// TestBearerAuthIsNotSubjectToOriginCheck 验证 APP 用 Bearer 时不受同源限制。
//
// Bearer 不是浏览器自动附带的凭据，不存在 CSRF 面；cookie 路径的同源校验
// 由 internal/api 的单测覆盖。
func TestBearerAuthIsNotSubjectToOriginCheck(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")
	session := env.AuthorizeApp(browser, "device-1")

	app := env.NewClient().WithBearer(session.AccessToken).WithHeader("Origin", "")
	app.Patch("/api/v1/me", map[string]string{"display_name": "From App"}).
		ExpectStatus(http.StatusOK)
}

func TestSessionsRequireAuth(t *testing.T) {
	env := testsupport.New(t)

	for _, path := range []string{
		"/api/v1/me", "/api/v1/me/sessions", "/api/v1/me/referrals", "/api/v1/me/entitlement",
	} {
		env.NewClient().Get(path).ExpectStatus(http.StatusUnauthorized)
	}
}
