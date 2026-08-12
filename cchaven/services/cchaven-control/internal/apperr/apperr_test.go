package apperr

import (
	"errors"
	"net/http"
	"testing"
	"time"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/i18n"
)

// TestRateLimitedRendersSection62Copy 验证限频错误渲染出 6.2 节的固定模板，
// 且单位在秒与分钟之间正确切换。
func TestRateLimitedRendersSection62Copy(t *testing.T) {
	cases := []struct {
		name       string
		retryAfter time.Duration
		want       string
	}{
		{"不足一分钟用秒", 42 * time.Second, "尝试次数过多，请 42 秒后再试。"},
		{"整一分钟用分钟", time.Minute, "尝试次数过多，请 1 分钟后再试。"},
		{"九十秒向上取整", 90 * time.Second, "尝试次数过多，请 2 分钟后再试。"},
		{"零时长兜底为一秒", 0, "尝试次数过多，请 1 秒后再试。"},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			err := RateLimited(tc.retryAfter)
			if got := i18n.T(i18n.ZhCN, err.Message, err.Args); got != tc.want {
				t.Errorf("文案 = %q, want %q", got, tc.want)
			}
			if err.Status != http.StatusTooManyRequests {
				t.Errorf("状态码 = %d, want 429", err.Status)
			}
		})
	}
}

func TestAccountLockedRendersMinutes(t *testing.T) {
	err := AccountLocked(15 * time.Minute)

	want := "尝试次数过多，请 15 分钟后再试。"
	if got := i18n.T(i18n.ZhCN, err.Message, err.Args); got != want {
		t.Errorf("文案 = %q, want %q", got, want)
	}
	if err.Status != http.StatusLocked {
		t.Errorf("状态码 = %d, want 423", err.Status)
	}
}

func TestCodeInvalidCarriesRemainingAttempts(t *testing.T) {
	err := CodeInvalid(3)

	want := "验证码不正确，还剩 3 次尝试机会。"
	if got := i18n.T(i18n.ZhCN, err.Message, err.Args); got != want {
		t.Errorf("文案 = %q, want %q", got, want)
	}
	if err.Details["attempts_remaining"] != 3 {
		t.Errorf("details.attempts_remaining = %v, want 3", err.Details["attempts_remaining"])
	}
}

// TestAuthMigratedPointsAtTheAccountCentre 锁住自有认证接口下线后的契约。
//
// 存量客户端仍会打这些端点，必须拿到「为什么没了、去哪」的可解释响应：
// 410 表示资源永久消失（区别于 404 的「可能拼错了」），reason 稳定供机器分支，
// portal_url 让前端能直接把用户送到账号中心。
func TestAuthMigratedPointsAtTheAccountCentre(t *testing.T) {
	err := AuthMigrated("https://lumiogame.com/login")

	if err.Status != http.StatusGone {
		t.Errorf("状态码 = %d, want 410", err.Status)
	}
	if err.Code != "auth_migrated" {
		t.Errorf("错误码 = %q, want auth_migrated", err.Code)
	}
	if got := err.Details["portal_url"]; got != "https://lumiogame.com/login" {
		t.Errorf("details.portal_url = %v", got)
	}
	if got := err.Details["reason"]; got != "identity_moved_to_lumio" {
		t.Errorf("details.reason = %v, want identity_moved_to_lumio", got)
	}
	want := "账号体系已统一到 Lumio 账号中心，请前往 https://lumiogame.com/login 登录。"
	if got := i18n.T(i18n.ZhCN, err.Message, err.Args); got != want {
		t.Errorf("文案 = %q, want %q", got, want)
	}
}

// TestIdentityUnavailableIsA503 锁住降级策略：Sub2API 不可用时必须明确失败。
// 用 5xx 而不是 401，前端才不会把「上游抖动」误判成「登录失效」把用户踢出去。
func TestIdentityUnavailableIsA503(t *testing.T) {
	err := IdentityUnavailable()

	if err.Status != http.StatusServiceUnavailable {
		t.Errorf("状态码 = %d, want 503", err.Status)
	}
	if err.Code != "identity_unavailable" {
		t.Errorf("错误码 = %q, want identity_unavailable", err.Code)
	}
	if got := i18n.T(i18n.ZhCN, err.Message, nil); got != "账号服务暂时不可用，请稍后重试。" {
		t.Errorf("文案 = %q", got)
	}
}

// TestFromFoldsUnknownErrors 确认非本包错误不会把内部细节泄露到响应里。
func TestFromFoldsUnknownErrors(t *testing.T) {
	internal := errors.New("pq: relation \"users\" does not exist")

	converted := From(internal)
	if converted.Status != http.StatusInternalServerError {
		t.Errorf("状态码 = %d, want 500", converted.Status)
	}
	if converted.Code != "internal_error" {
		t.Errorf("错误码 = %q, want internal_error", converted.Code)
	}
	if got := i18n.T(i18n.ZhCN, converted.Message, nil); got != "服务暂时不可用，请稍后重试。" {
		t.Errorf("对外文案不应暴露内部细节, got %q", got)
	}
	// 内部原因仍可通过 errors.Is 取到，供日志使用。
	if !errors.Is(converted, internal) {
		t.Error("应保留内部错误原因供日志排查")
	}
}

func TestFromPreservesAppError(t *testing.T) {
	original := InvalidCredentials()
	if got := From(original); got != original {
		t.Error("已是 *Error 时应原样返回")
	}
}

// TestWithDetailDoesNotMutateOriginal 确认错误构造器返回的是副本，
// 避免包级共享的错误实例被意外污染。
func TestWithDetailDoesNotMutateOriginal(t *testing.T) {
	base := NotFound()
	derived := base.WithDetail("resource", "order")

	if base.Details != nil {
		t.Error("原错误不应被修改")
	}
	if derived.Details["resource"] != "order" {
		t.Errorf("副本应带上细节, got %v", derived.Details)
	}
}
