package test

import (
	"net/http"
	"slices"
	"testing"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/store"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/testsupport"
)

func TestMeReturnsDisplayIDAndEntitlement(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com", "Passw0rd!")

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
	browser, _ := env.SignUp("alice@example.com", "Passw0rd!")

	resp := browser.Patch("/api/v1/me", map[string]string{"display_name": "Mary"}).
		ExpectStatus(http.StatusOK)

	if got := resp.String("display_name"); got != "Mary" {
		t.Errorf("显示名称 = %q", got)
	}
	if got := browser.Get("/api/v1/me").Object("user")["display_name"]; got != "Mary" {
		t.Errorf("重新读取的显示名称 = %v", got)
	}
}

// TestChangePasswordKeepsCurrentSession 验证「密码已更新，其他设备已退出登录。」
func TestChangePasswordKeepsCurrentSession(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com", "Passw0rd!")
	session := env.AuthorizeApp(browser, "device-1")
	appClient := env.NewClient().WithBearer(session.AccessToken)

	resp := browser.Post("/api/v1/me/password", map[string]string{
		"current_password": "Passw0rd!", "new_password": "NewPassw0rd!",
	}).ExpectStatus(http.StatusOK)

	if got := resp.String("message"); got != "密码已更新，其他设备已退出登录。" {
		t.Errorf("文案 = %q", got)
	}

	// 当前会话保留，其他会话被撤销。
	browser.Get("/api/v1/me").ExpectStatus(http.StatusOK)
	appClient.Get("/api/v1/me").ExpectStatus(http.StatusUnauthorized)

	env.NewClient().Post("/api/v1/auth/login", map[string]string{
		"email": "alice@example.com", "password": "NewPassw0rd!",
	}).ExpectStatus(http.StatusOK)
}

func TestChangePasswordRejectsWrongCurrent(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com", "Passw0rd!")

	resp := browser.Post("/api/v1/me/password", map[string]string{
		"current_password": "WrongPass1", "new_password": "NewPassw0rd!",
	}).ExpectStatus(http.StatusBadRequest)

	if got := resp.ErrorMessage(); got != "当前密码不正确。" {
		t.Errorf("文案 = %q", got)
	}
}

// TestEmailChangeTwoStep 验证改邮箱的两步流程与原邮箱通知。
func TestEmailChangeTwoStep(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com", "Passw0rd!")

	request := browser.Post("/api/v1/me/email-change", map[string]string{
		"new_email": "alice.new@example.com",
	}).ExpectStatus(http.StatusAccepted)

	code := request.String("dev_code")
	if code == "" {
		t.Fatal("应向新邮箱发送验证码")
	}
	// 验证码发往新邮箱，不是原邮箱。
	if templates := env.OutboxTemplates("alice.new@example.com"); !slices.Contains(templates, store.TemplateEmailChange) {
		t.Errorf("新邮箱应收到验证码, 实际 %v", templates)
	}

	// 错误验证码带剩余次数。
	wrong := browser.Post("/api/v1/me/email-change/verify", map[string]string{"code": "000000"}).
		ExpectStatus(http.StatusBadRequest)
	if got := wrong.ErrorMessage(); got != "验证码不正确，还剩 4 次尝试机会。" {
		t.Errorf("文案 = %q", got)
	}

	browser.Post("/api/v1/me/email-change/verify", map[string]string{"code": code}).
		ExpectStatus(http.StatusOK)

	if got := browser.Get("/api/v1/me").Object("user")["email"]; got != "alice.new@example.com" {
		t.Errorf("邮箱未切换, got %v", got)
	}
	// 原邮箱收到变更通知。
	if templates := env.OutboxTemplates("alice@example.com"); !slices.Contains(templates, store.TemplateEmailChanged) {
		t.Errorf("原邮箱应收到变更通知, 实际 %v", templates)
	}

	// 新邮箱可用于登录。
	env.NewClient().Post("/api/v1/auth/login", map[string]string{
		"email": "alice.new@example.com", "password": "Passw0rd!",
	}).ExpectStatus(http.StatusOK)
}

func TestEmailChangeRejectsTakenAddress(t *testing.T) {
	env := testsupport.New(t)
	env.SignUp("taken@example.com", "Passw0rd!")
	browser, _ := env.SignUp("alice@example.com", "Passw0rd!")

	resp := browser.Post("/api/v1/me/email-change", map[string]string{
		"new_email": "taken@example.com",
	}).ExpectStatus(http.StatusConflict)

	if got := resp.ErrorMessage(); got != "该邮箱已注册。" {
		t.Errorf("文案 = %q", got)
	}
}

func TestEmailChangeCanBeCancelled(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com", "Passw0rd!")

	code := browser.Post("/api/v1/me/email-change", map[string]string{
		"new_email": "alice.new@example.com",
	}).ExpectStatus(http.StatusAccepted).String("dev_code")

	browser.Delete("/api/v1/me/email-change").ExpectStatus(http.StatusNoContent)

	// 取消后原验证码作废。
	browser.Post("/api/v1/me/email-change/verify", map[string]string{"code": code}).
		ExpectStatus(http.StatusGone)
}

// TestAccountDeletionHasSevenDayGracePeriod 验证注销 7 天冷静期与撤销。
func TestAccountDeletionHasSevenDayGracePeriod(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com", "Passw0rd!")

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

// TestLogoutRevokesSession 验证退出登录同时清 cookie 与撤销服务端会话。
func TestLogoutRevokesSession(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com", "Passw0rd!")

	browser.Post("/api/v1/auth/logout", nil).ExpectStatus(http.StatusNoContent)
	browser.Get("/api/v1/me").ExpectStatus(http.StatusUnauthorized)
}

// TestCookieAuthRequiresTrustedOrigin 验证 cookie 鉴权的写操作有 CSRF 防护。
func TestCookieAuthRequiresTrustedOrigin(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com", "Passw0rd!")

	// 来自第三方站点的写请求被拒。
	evil := browser.WithHeader("Origin", "https://evil.example.com")
	evil.Patch("/api/v1/me", map[string]string{"display_name": "Hacked"}).
		ExpectStatus(http.StatusForbidden)

	// 读请求不改变状态，放行。
	evil.Get("/api/v1/me").ExpectStatus(http.StatusOK)

	if got := browser.Get("/api/v1/me").Object("user")["display_name"]; got != "" {
		t.Errorf("跨站请求不应改到数据, got %v", got)
	}
}

// TestBearerAuthIsNotSubjectToOriginCheck 验证 APP 用 Bearer 时不受同源限制。
func TestBearerAuthIsNotSubjectToOriginCheck(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com", "Passw0rd!")
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
