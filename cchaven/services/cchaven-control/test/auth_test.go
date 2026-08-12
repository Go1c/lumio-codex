package test

import (
	"net/http"
	"testing"
	"time"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/service"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/testsupport"
)

// TestRegisterDoesNotIssueSession 验证注册成功后处于 pending_email 状态，
// 不发放任何可用会话（交互设计 3.1）。
func TestRegisterDoesNotIssueSession(t *testing.T) {
	env := testsupport.New(t)
	client := env.NewClient()

	env.Register(client, "alice@example.com", "Passw0rd!")

	// 带着注册后的 cookie 访问受保护接口应被拒绝。
	client.Get("/api/v1/me").ExpectStatus(http.StatusUnauthorized)

	var status string
	if err := env.Pool.QueryRow(t.Context(),
		`SELECT status FROM users WHERE email = 'alice@example.com'`).Scan(&status); err != nil {
		t.Fatalf("查询用户状态失败: %v", err)
	}
	if status != "pending_email" {
		t.Errorf("注册后状态 = %q, want pending_email", status)
	}
}

func TestRegisterRejectsDuplicateEmail(t *testing.T) {
	env := testsupport.New(t)
	env.SignUp("alice@example.com", "Passw0rd!")

	resp := env.NewClient().Post("/api/v1/auth/register", map[string]string{
		"email": "alice@example.com", "password": "Passw0rd!",
	}).ExpectStatus(http.StatusConflict)

	if got := resp.ErrorMessage(); got != "该邮箱已注册。" {
		t.Errorf("文案 = %q, want 该邮箱已注册。", got)
	}
}

func TestRegisterRejectsWeakPassword(t *testing.T) {
	env := testsupport.New(t)

	for _, password := range []string{"short1", "abcdefgh", "12345678"} {
		resp := env.NewClient().Post("/api/v1/auth/register", map[string]string{
			"email": "alice@example.com", "password": password,
		}).ExpectStatus(http.StatusBadRequest)

		if resp.ErrorCode() != "password_too_weak" {
			t.Errorf("口令 %q 应被拒绝为弱口令, got %q", password, resp.ErrorCode())
		}
	}
}

// TestVerifyEmailAttemptCountdown 验证错误验证码逐次递减剩余次数，
// 文案严格遵循 6.2 节模板。
func TestVerifyEmailAttemptCountdown(t *testing.T) {
	env := testsupport.New(t)
	client := env.NewClient()
	env.Register(client, "alice@example.com", "Passw0rd!")

	wantMessages := []string{
		"验证码不正确，还剩 4 次尝试机会。",
		"验证码不正确，还剩 3 次尝试机会。",
		"验证码不正确，还剩 2 次尝试机会。",
		"验证码不正确，还剩 1 次尝试机会。",
	}

	for i, want := range wantMessages {
		resp := client.Post("/api/v1/auth/verify-email", map[string]string{
			"email": "alice@example.com", "code": "000000",
		}).ExpectStatus(http.StatusBadRequest)

		if got := resp.ErrorMessage(); got != want {
			t.Errorf("第 %d 次错误文案 = %q, want %q", i+1, got, want)
		}
	}

	// 第 5 次错误耗尽次数，转为「已过期，请重新发送」。
	resp := client.Post("/api/v1/auth/verify-email", map[string]string{
		"email": "alice@example.com", "code": "000000",
	}).ExpectStatus(http.StatusGone)

	if got := resp.ErrorMessage(); got != "该验证码已过期，请重新发送。" {
		t.Errorf("耗尽后文案 = %q", got)
	}
}

func TestVerifyEmailRejectsExpiredCode(t *testing.T) {
	env := testsupport.New(t)
	client := env.NewClient()
	code := env.Register(client, "alice@example.com", "Passw0rd!")

	env.Advance(service.VerificationCodeTTL + time.Minute)

	resp := client.Post("/api/v1/auth/verify-email", map[string]string{
		"email": "alice@example.com", "code": code,
	}).ExpectStatus(http.StatusGone)

	if got := resp.ErrorMessage(); got != "该验证码已过期，请重新发送。" {
		t.Errorf("文案 = %q", got)
	}
}

// TestResendVerificationCooldown 验证 60 秒重发冷却。
// 规范 3.1 与 6.2 均为 60 秒；原型的 10 秒只是演示压缩。
func TestResendVerificationCooldown(t *testing.T) {
	env := testsupport.New(t)
	client := env.NewClient()
	first := env.Register(client, "alice@example.com", "Passw0rd!")

	// 冷却期内重发：不下发新码，返回剩余秒数。
	resp := client.Post("/api/v1/auth/verification-code/resend", map[string]string{
		"email": "alice@example.com",
	}).ExpectStatus(http.StatusAccepted)

	if retry := resp.Number("retry_after_seconds"); retry <= 0 || retry > 60 {
		t.Errorf("冷却剩余秒数不合理: %v", retry)
	}
	if resp.String("dev_code") != "" {
		t.Error("冷却期内不应生成新验证码")
	}

	// 冷却结束后可以重发，且旧码立即失效。
	env.Advance(service.VerificationResendCooldown + time.Second)
	resend := client.Post("/api/v1/auth/verification-code/resend", map[string]string{
		"email": "alice@example.com",
	}).ExpectStatus(http.StatusAccepted)

	second := resend.String("dev_code")
	if second == "" {
		t.Fatal("冷却结束后应下发新验证码")
	}

	client.Post("/api/v1/auth/verify-email", map[string]string{
		"email": "alice@example.com", "code": first,
	}).ExpectStatus(http.StatusBadRequest)

	client.Post("/api/v1/auth/verify-email", map[string]string{
		"email": "alice@example.com", "code": second,
	}).ExpectStatus(http.StatusOK)
}

// TestResendForUnknownEmailSucceedsSilently 验证重发接口不成为账号存在性探针。
func TestResendForUnknownEmailSucceedsSilently(t *testing.T) {
	env := testsupport.New(t)

	env.NewClient().Post("/api/v1/auth/verification-code/resend", map[string]string{
		"email": "nobody@example.com",
	}).ExpectStatus(http.StatusAccepted)
}

// TestLoginUsesUniformFailureCopy 验证「邮箱不存在」与「密码错误」返回完全一致的响应，
// 这是 6.2 节防枚举的核心要求。
func TestLoginUsesUniformFailureCopy(t *testing.T) {
	env := testsupport.New(t)
	env.SignUp("alice@example.com", "Passw0rd!")

	unknown := env.NewClient().Post("/api/v1/auth/login", map[string]string{
		"email": "nobody@example.com", "password": "Passw0rd!",
	}).ExpectStatus(http.StatusUnauthorized)

	wrongPassword := env.NewClient().Post("/api/v1/auth/login", map[string]string{
		"email": "alice@example.com", "password": "WrongPass1",
	}).ExpectStatus(http.StatusUnauthorized)

	if unknown.ErrorCode() != wrongPassword.ErrorCode() {
		t.Errorf("错误码应一致: %q vs %q", unknown.ErrorCode(), wrongPassword.ErrorCode())
	}
	if got := unknown.ErrorMessage(); got != "邮箱或密码不正确。" {
		t.Errorf("文案 = %q, want 邮箱或密码不正确。", got)
	}
	if unknown.ErrorMessage() != wrongPassword.ErrorMessage() {
		t.Error("两种失败原因的文案必须完全一致")
	}
}

// TestLoginLocksAfterFiveFailures 验证 5 次失败锁定 15 分钟。
func TestLoginLocksAfterFiveFailures(t *testing.T) {
	env := testsupport.New(t)
	env.SignUp("alice@example.com", "Passw0rd!")

	client := env.NewClient()
	for range service.LoginFailureThreshold - 1 {
		client.Post("/api/v1/auth/login", map[string]string{
			"email": "alice@example.com", "password": "WrongPass1",
		}).ExpectStatus(http.StatusUnauthorized)
	}

	locked := client.Post("/api/v1/auth/login", map[string]string{
		"email": "alice@example.com", "password": "WrongPass1",
	}).ExpectStatus(http.StatusLocked)

	if got := locked.ErrorMessage(); got != "尝试次数过多，请 15 分钟后再试。" {
		t.Errorf("锁定文案 = %q", got)
	}

	// 锁定期内即便口令正确也不放行。
	client.Post("/api/v1/auth/login", map[string]string{
		"email": "alice@example.com", "password": "Passw0rd!",
	}).ExpectStatus(http.StatusLocked)

	// 锁定期满后恢复。
	env.Advance(service.LoginLockDuration + time.Minute)
	client.Post("/api/v1/auth/login", map[string]string{
		"email": "alice@example.com", "password": "Passw0rd!",
	}).ExpectStatus(http.StatusOK)
}

// TestLoginRejectsUnverifiedEmail 验证未验证邮箱在口令正确时才暴露，
// 避免该提示沦为账号存在性探针。
func TestLoginRejectsUnverifiedEmail(t *testing.T) {
	env := testsupport.New(t)
	client := env.NewClient()
	env.Register(client, "alice@example.com", "Passw0rd!")

	resp := client.Post("/api/v1/auth/login", map[string]string{
		"email": "alice@example.com", "password": "Passw0rd!",
	}).ExpectStatus(http.StatusForbidden)

	if resp.ErrorCode() != "email_unverified" {
		t.Errorf("错误码 = %q, want email_unverified", resp.ErrorCode())
	}

	// 口令错误时仍然只返回统一的凭据错误。
	wrong := client.Post("/api/v1/auth/login", map[string]string{
		"email": "alice@example.com", "password": "WrongPass1",
	}).ExpectStatus(http.StatusUnauthorized)
	if wrong.ErrorCode() != "invalid_credentials" {
		t.Errorf("错误码 = %q, want invalid_credentials", wrong.ErrorCode())
	}
}

// TestForgotPasswordAlwaysSucceeds 验证忘记密码的恒定回执文案。
func TestForgotPasswordAlwaysSucceeds(t *testing.T) {
	env := testsupport.New(t)
	env.SignUp("alice@example.com", "Passw0rd!")

	registered := env.NewClient().Post("/api/v1/auth/password/forgot", map[string]string{
		"email": "alice@example.com",
	}).ExpectStatus(http.StatusAccepted)

	unknown := env.NewClient().Post("/api/v1/auth/password/forgot", map[string]string{
		"email": "nobody@example.com",
	}).ExpectStatus(http.StatusAccepted)

	if got := registered.String("message"); got != "如 alice@example.com 已注册账号，你将很快收到重设链接。" {
		t.Errorf("文案 = %q", got)
	}
	if got := unknown.String("message"); got != "如 nobody@example.com 已注册账号，你将很快收到重设链接。" {
		t.Errorf("未注册邮箱应返回同样格式的回执, got %q", got)
	}
	// 未注册邮箱不应真的产生令牌。
	if unknown.String("dev_token") != "" {
		t.Error("未注册邮箱不应生成重设令牌")
	}
}

// TestResetPasswordIsOneTimeAndRevokesAllSessions 验证重设链接一次性，
// 且成功后撤销该账号全部会话（交互设计 3.3）。
func TestResetPasswordIsOneTimeAndRevokesAllSessions(t *testing.T) {
	env := testsupport.New(t)
	browser, userID := env.SignUp("alice@example.com", "Passw0rd!")

	// 建立一个 APP 会话，稍后验证它也被撤销。
	app := env.AuthorizeApp(browser, "device-1")
	appClient := env.NewClient().WithBearer(app.AccessToken)
	appClient.Get("/api/v1/me").ExpectStatus(http.StatusOK)

	forgot := env.NewClient().Post("/api/v1/auth/password/forgot", map[string]string{
		"email": "alice@example.com",
	}).ExpectStatus(http.StatusAccepted)
	token := forgot.String("dev_token")
	if token == "" {
		t.Fatal("应下发重设令牌")
	}

	env.NewClient().Get("/api/v1/auth/password/reset/" + token).ExpectStatus(http.StatusOK)

	reset := env.NewClient().Post("/api/v1/auth/password/reset", map[string]string{
		"token": token, "password": "NewPassw0rd!",
	}).ExpectStatus(http.StatusOK)
	if got := reset.String("message"); got != "密码已更新，所有设备已退出登录。" {
		t.Errorf("文案 = %q", got)
	}

	// 一次性：同一令牌不可复用。
	replay := env.NewClient().Post("/api/v1/auth/password/reset", map[string]string{
		"token": token, "password": "AnotherPass1",
	}).ExpectStatus(http.StatusGone)
	if got := replay.ErrorMessage(); got != "该链接已过期或已被使用。" {
		t.Errorf("文案 = %q", got)
	}

	// 全部会话被撤销。
	browser.Get("/api/v1/me").ExpectStatus(http.StatusUnauthorized)
	appClient.Get("/api/v1/me").ExpectStatus(http.StatusUnauthorized)

	var active int
	if err := env.Pool.QueryRow(t.Context(),
		`SELECT count(*) FROM session_families WHERE user_id = $1 AND revoked_at IS NULL`,
		userID).Scan(&active); err != nil {
		t.Fatalf("查询会话失败: %v", err)
	}
	if active != 0 {
		t.Errorf("重设密码后不应有活跃会话，got %d", active)
	}

	// 新口令可用，旧口令不可用。
	env.NewClient().Post("/api/v1/auth/login", map[string]string{
		"email": "alice@example.com", "password": "Passw0rd!",
	}).ExpectStatus(http.StatusUnauthorized)
	env.NewClient().Post("/api/v1/auth/login", map[string]string{
		"email": "alice@example.com", "password": "NewPassw0rd!",
	}).ExpectStatus(http.StatusOK)
}

func TestResetTokenExpires(t *testing.T) {
	env := testsupport.New(t)
	env.SignUp("alice@example.com", "Passw0rd!")

	forgot := env.NewClient().Post("/api/v1/auth/password/forgot", map[string]string{
		"email": "alice@example.com",
	}).ExpectStatus(http.StatusAccepted)

	env.Advance(service.PasswordResetTTL + time.Minute)

	env.NewClient().Get("/api/v1/auth/password/reset/" + forgot.String("dev_token")).
		ExpectStatus(http.StatusGone)
}

// TestPublicConfigDrivesFrontendCopy 验证价格与邀请规则由后台下发，页面不写死。
func TestPublicConfigDrivesFrontendCopy(t *testing.T) {
	env := testsupport.New(t)

	resp := env.NewClient().Get("/api/v1/config/public").ExpectStatus(http.StatusOK)

	pricing := resp.Object("pricing")
	if pricing["amount_cents"] != float64(6800) || pricing["currency"] != "CNY" {
		t.Errorf("价格应来自运营配置, got %v", pricing)
	}

	invite := resp.Object("invite")
	if invite["reward_days"] != float64(7) || invite["trial_days"] != float64(30) {
		t.Errorf("邀请配置不符, got %v", invite)
	}
	if invite["reward_enabled"] != true {
		t.Error("奖励天数大于 0 时 reward_enabled 应为 true")
	}

	// 改配置后立即生效。
	env.SetOpsConfig("pricing.monthly", `{"amount_cents": 9900, "currency": "CNY"}`)
	env.SetOpsConfig("invite.reward_days", "0")

	updated := env.NewClient().Get("/api/v1/config/public").ExpectStatus(http.StatusOK)
	if updated.Object("pricing")["amount_cents"] != float64(9900) {
		t.Error("价格变更应实时生效")
	}
	if updated.Object("invite")["reward_enabled"] != false {
		t.Error("奖励天数配 0 时 reward_enabled 应为 false")
	}
}

// TestInvalidInviteCodeDoesNotBlockSignup 验证失效邀请码不阻断注册转化（4.4）。
func TestInvalidInviteCodeDoesNotBlockSignup(t *testing.T) {
	env := testsupport.New(t)

	client := env.NewClient()
	landing := client.Get("/api/v1/invites/nonexistent").ExpectStatus(http.StatusOK)
	if landing.Data()["valid"] != false {
		t.Errorf("不存在的邀请码应返回 valid=false, got %s", landing.Raw)
	}

	env.Register(client, "alice@example.com", "Passw0rd!")
	userID := env.UserIDOf("alice@example.com")
	assertRegistrationSource(t, env, userID, "organic")
}
