package test

import (
	"net/http"
	"testing"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/testsupport"
)

// TestSelfServeAuthEndpointsAreGone 端到端锁住自有终端用户认证的下线。
//
// 邮箱、口令、验证码全部归 Lumio 账号中心（Sub2API）。这些端点保留路由但回 410，
// 存量客户端才分得清「这个能力永久搬走了」与「路径写错了」，
// 并能从 details.portal_url 直接把用户送到账号中心。
func TestSelfServeAuthEndpointsAreGone(t *testing.T) {
	env := testsupport.New(t)
	client := env.NewClient()

	calls := []struct {
		name string
		do   func() *testsupport.Response
	}{
		{"注册", func() *testsupport.Response {
			return client.Post("/api/v1/auth/register",
				map[string]string{"email": "alice@example.com", "password": "Passw0rd!"})
		}},
		{"验证邮箱", func() *testsupport.Response {
			return client.Post("/api/v1/auth/verify-email",
				map[string]string{"email": "alice@example.com", "code": "000000"})
		}},
		{"重发验证码", func() *testsupport.Response {
			return client.Post("/api/v1/auth/verification-code/resend",
				map[string]string{"email": "alice@example.com"})
		}},
		{"登录", func() *testsupport.Response {
			return client.Post("/api/v1/auth/login",
				map[string]string{"email": "alice@example.com", "password": "Passw0rd!"})
		}},
		{"忘记密码", func() *testsupport.Response {
			return client.Post("/api/v1/auth/password/forgot",
				map[string]string{"email": "alice@example.com"})
		}},
		{"查看重设链接", func() *testsupport.Response {
			return client.Get("/api/v1/auth/password/reset/whatever")
		}},
		{"重设密码", func() *testsupport.Response {
			return client.Post("/api/v1/auth/password/reset",
				map[string]string{"token": "whatever", "password": "NewPassw0rd!"})
		}},
		{"官网会话续期", func() *testsupport.Response {
			return client.Post("/api/v1/auth/refresh", map[string]string{})
		}},
		{"修改密码", func() *testsupport.Response {
			return client.Post("/api/v1/me/password",
				map[string]string{"current_password": "Passw0rd!", "new_password": "NewPassw0rd!"})
		}},
		{"申请改邮箱", func() *testsupport.Response {
			return client.Post("/api/v1/me/email-change",
				map[string]string{"new_email": "new@example.com"})
		}},
	}

	for _, tc := range calls {
		t.Run(tc.name, func(t *testing.T) {
			resp := tc.do().ExpectStatus(http.StatusGone)

			if resp.ErrorCode() != "auth_migrated" {
				t.Errorf("错误码 = %q, want auth_migrated", resp.ErrorCode())
			}
			if got := resp.ErrorDetail("portal_url"); got != "https://portal.test/login" {
				t.Errorf("details.portal_url = %v", got)
			}
		})
	}
}

// TestNoUserIsCreatedByTheRetiredRegisterEndpoint 确认 410 是真的短路，
// 没有在返回错误之前顺手写库。
func TestNoUserIsCreatedByTheRetiredRegisterEndpoint(t *testing.T) {
	env := testsupport.New(t)

	env.NewClient().Post("/api/v1/auth/register", map[string]string{
		"email": "alice@example.com", "password": "Passw0rd!",
	}).ExpectStatus(http.StatusGone)

	var count int
	if err := env.Pool.QueryRow(t.Context(),
		`SELECT count(*) FROM users`).Scan(&count); err != nil {
		t.Fatalf("查询用户失败: %v", err)
	}
	if count != 0 {
		t.Errorf("下线的注册端点不应建号, got %d 条", count)
	}
}

// TestPublicConfigDrivesFrontendCopy 验证价格与邀请规则由后台下发，页面不写死。
func TestPublicConfigDrivesFrontendCopy(t *testing.T) {
	env := testsupport.New(t)

	resp := env.NewClient().Get("/api/v1/config/public").ExpectStatus(http.StatusOK)

	pricing := resp.Object("pricing")
	if pricing["amount_cents"] != float64(1990) || pricing["currency"] != "CNY" {
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

// TestInvalidInviteCodeDoesNotBlockSignup 验证失效邀请码不阻断开户转化（4.4）。
func TestInvalidInviteCodeDoesNotBlockSignup(t *testing.T) {
	env := testsupport.New(t)

	client := env.NewClient()
	landing := client.Get("/api/v1/invites/nonexistent").ExpectStatus(http.StatusOK)
	if landing.Data()["valid"] != false {
		t.Errorf("不存在的邀请码应返回 valid=false, got %s", landing.Raw)
	}

	_, userID := env.Identify(client, "alice@example.com")
	assertRegistrationSource(t, env, userID, "organic")
}
