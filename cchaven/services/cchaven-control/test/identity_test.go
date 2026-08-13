package test

import (
	"net/http"
	"testing"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/testsupport"
)

// identityOf 读出某个本地用户绑定的 Sub2API 用户 ID（映射表与 users 冗余列必须一致）。
func identityOf(t *testing.T, env *testsupport.Env, userID int64) (mapped, redundant string) {
	t.Helper()

	if err := env.Pool.QueryRow(t.Context(),
		`SELECT sub2api_user_id FROM sub2api_identities WHERE user_id = $1`, userID).
		Scan(&mapped); err != nil {
		t.Fatalf("查询身份映射失败: %v", err)
	}
	var column *string
	if err := env.Pool.QueryRow(t.Context(),
		`SELECT sub2api_user_id FROM users WHERE id = $1`, userID).Scan(&column); err != nil {
		t.Fatalf("查询 users.sub2api_user_id 失败: %v", err)
	}
	if column != nil {
		redundant = *column
	}
	return mapped, redundant
}

// TestFirstRequestProvisionsAShadowAccount 验证「Sub2API 用户首次出现即开户」。
//
// 注册已经不在本服务发生，能观察到的第一件事就是一个带令牌的请求；
// 影子账号在那一刻建立，且必须直接是 active——邮箱已由账号中心验证过，
// 本地再要一次验证码毫无意义。
func TestFirstRequestProvisionsAShadowAccount(t *testing.T) {
	env := testsupport.New(t)

	token := env.Sub2API.Issue("alice@example.com")
	client := env.NewClient().WithBearer(token)

	me := client.Get("/api/v1/me").ExpectStatus(http.StatusOK)
	if got := me.Object("user")["email"]; got != "alice@example.com" {
		t.Errorf("user.email = %v", got)
	}

	userID := env.UserIDOf("alice@example.com")
	var status, passwordHash string
	if err := env.Pool.QueryRow(t.Context(),
		`SELECT status, password_hash FROM users WHERE id = $1`, userID).
		Scan(&status, &passwordHash); err != nil {
		t.Fatalf("查询用户失败: %v", err)
	}
	if status != "active" {
		t.Errorf("开户状态 = %q, want active", status)
	}
	// 影子账号没有本地口令，只有一个永远匹配不上的占位值。
	if passwordHash == "" || len(passwordHash) > 32 {
		t.Errorf("password_hash 应为占位值, got %q", passwordHash)
	}

	mapped, redundant := identityOf(t, env, userID)
	if mapped != env.Sub2API.UserIDOf(token) {
		t.Errorf("映射的 Sub2API 用户 ID = %q, want %q", mapped, env.Sub2API.UserIDOf(token))
	}
	if redundant != mapped {
		t.Errorf("users.sub2api_user_id = %q, 应与映射表一致 (%q)", redundant, mapped)
	}

	// 开户即拥有自己的邀请码，账户中心可直接展示。
	if code := env.ReferralCodeOf(userID); code == "" {
		t.Error("开户后应生成邀请码")
	}
}

// TestSameIdentityReusesTheLocalAccount 换设备、换令牌都还是同一个本地账号。
func TestSameIdentityReusesTheLocalAccount(t *testing.T) {
	env := testsupport.New(t)

	first := env.Sub2API.Issue("alice@example.com")
	env.NewClient().WithBearer(first).Get("/api/v1/me").ExpectStatus(http.StatusOK)
	userID := env.UserIDOf("alice@example.com")

	second := env.Sub2API.IssueFor(env.Sub2API.UserIDOf(first), "alice@example.com")
	env.NewClient().WithBearer(second).Get("/api/v1/me").ExpectStatus(http.StatusOK)

	var count int
	if err := env.Pool.QueryRow(t.Context(), `SELECT count(*) FROM users`).Scan(&count); err != nil {
		t.Fatalf("查询用户失败: %v", err)
	}
	if count != 1 {
		t.Fatalf("同一身份不应产生第二个本地账号, got %d 条", count)
	}
	if got := env.UserIDOf("alice@example.com"); got != userID {
		t.Errorf("本地用户 ID 变了: %d -> %d", userID, got)
	}
}

// TestEmailChangedUpstreamSyncsLocally 账号中心改了邮箱，本地要跟上。
// 后台检索与邀请邮件都读本地这份，落后会让运营查不到人。
func TestEmailChangedUpstreamSyncsLocally(t *testing.T) {
	env := testsupport.New(t)

	token := env.Sub2API.Issue("alice@example.com")
	client := env.NewClient().WithBearer(token)
	client.Get("/api/v1/me").ExpectStatus(http.StatusOK)
	userID := env.UserIDOf("alice@example.com")

	env.Sub2API.SetEmail(token, "alice+new@example.com")

	me := client.Get("/api/v1/me").ExpectStatus(http.StatusOK)
	if got := me.Object("user")["email"]; got != "alice+new@example.com" {
		t.Errorf("user.email = %v, want alice+new@example.com", got)
	}
	if got := env.UserIDOf("alice+new@example.com"); got != userID {
		t.Errorf("改邮箱不应换账号: %d -> %d", userID, got)
	}
}

// TestExistingLocalAccountIsClaimedByEmail 存量用户按邮箱认领，不新建、不丢历史。
//
// 这是迁移期最要紧的一条：老用户的订阅、邀请、订单都挂在原来的 users 行上，
// 首次带着 Sub2API 令牌回来时必须落到同一行。
func TestExistingLocalAccountIsClaimedByEmail(t *testing.T) {
	env := testsupport.New(t)

	// 先造一个「迁移前就存在」的账号：走一次开户，然后把映射抹掉。
	legacy := env.Sub2API.Issue("alice@example.com")
	env.NewClient().WithBearer(legacy).Get("/api/v1/me").ExpectStatus(http.StatusOK)
	userID := env.UserIDOf("alice@example.com")

	if _, err := env.Pool.Exec(t.Context(),
		`DELETE FROM sub2api_identities WHERE user_id = $1`, userID); err != nil {
		t.Fatalf("清除映射失败: %v", err)
	}
	if _, err := env.Pool.Exec(t.Context(),
		`UPDATE users SET sub2api_user_id = NULL WHERE id = $1`, userID); err != nil {
		t.Fatalf("清除冗余列失败: %v", err)
	}

	fresh := env.Sub2API.Issue("alice@example.com")
	env.NewClient().WithBearer(fresh).Get("/api/v1/me").ExpectStatus(http.StatusOK)

	var count int
	if err := env.Pool.QueryRow(t.Context(), `SELECT count(*) FROM users`).Scan(&count); err != nil {
		t.Fatalf("查询用户失败: %v", err)
	}
	if count != 1 {
		t.Fatalf("同邮箱的存量账号应被认领而不是新建, got %d 条", count)
	}
	if mapped, _ := identityOf(t, env, userID); mapped != env.Sub2API.UserIDOf(fresh) {
		t.Errorf("认领后的映射 = %q, want %q", mapped, env.Sub2API.UserIDOf(fresh))
	}
}

// TestUnknownTokenIsRejected 令牌不被账号中心认可时返回 401。
func TestUnknownTokenIsRejected(t *testing.T) {
	env := testsupport.New(t)

	resp := env.NewClient().WithBearer("not-a-real-token").
		Get("/api/v1/me").ExpectStatus(http.StatusUnauthorized)
	if resp.ErrorCode() != "unauthorized" {
		t.Errorf("错误码 = %q, want unauthorized", resp.ErrorCode())
	}
}

// TestDisabledUpstreamAccountIsRejected 账号中心停用后立即失效。
func TestDisabledUpstreamAccountIsRejected(t *testing.T) {
	env := testsupport.New(t)

	token := env.Sub2API.Issue("alice@example.com")
	client := env.NewClient().WithBearer(token)
	client.Get("/api/v1/me").ExpectStatus(http.StatusOK)

	env.Sub2API.SetStatus(token, "disabled")

	resp := client.Get("/api/v1/me").ExpectStatus(http.StatusForbidden)
	if resp.ErrorCode() != "account_disabled" {
		t.Errorf("错误码 = %q, want account_disabled", resp.ErrorCode())
	}
}

// TestIdentityUpstreamOutageFailsClosed 锁死降级策略。
//
// 账号中心不可用时必须明确失败（503），既不能放行，也不能伪装成 401
// 把所有在线用户踢回登录页。
func TestIdentityUpstreamOutageFailsClosed(t *testing.T) {
	env := testsupport.New(t)

	token := env.Sub2API.Issue("alice@example.com")
	client := env.NewClient().WithBearer(token)
	client.Get("/api/v1/me").ExpectStatus(http.StatusOK)

	env.Sub2API.SetUnavailable(true)

	resp := client.Get("/api/v1/me").ExpectStatus(http.StatusServiceUnavailable)
	if resp.ErrorCode() != "identity_unavailable" {
		t.Errorf("错误码 = %q, want identity_unavailable", resp.ErrorCode())
	}

	env.Sub2API.SetUnavailable(false)
	client.Get("/api/v1/me").ExpectStatus(http.StatusOK)
}

// TestLocallyDisabledAccountStaysBlocked 运营在后台封禁的用户，
// 即便账号中心那边一切正常也进不来——CC 侧的处置权不能被上游覆盖。
func TestLocallyDisabledAccountStaysBlocked(t *testing.T) {
	env := testsupport.New(t)

	token := env.Sub2API.Issue("alice@example.com")
	client := env.NewClient().WithBearer(token)
	client.Get("/api/v1/me").ExpectStatus(http.StatusOK)

	if _, err := env.Pool.Exec(t.Context(),
		`UPDATE users SET status = 'disabled' WHERE lower(email) = 'alice@example.com'`); err != nil {
		t.Fatalf("停用用户失败: %v", err)
	}

	resp := client.Get("/api/v1/me").ExpectStatus(http.StatusForbidden)
	if resp.ErrorCode() != "account_disabled" {
		t.Errorf("错误码 = %q, want account_disabled", resp.ErrorCode())
	}
}
