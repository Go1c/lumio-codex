package test

import (
	"net/http"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/pquerna/otp/totp"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/service"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/testsupport"
)

const adminEmail = "ops@cchaven.cn"
const opsEmail = "ops2@cchaven.cn"
const supportEmail = "support@cchaven.cn"
const adminPassword = "AdminPass1!"

// newAdminClient 创建 owner 管理员并完成登录（未启用两步验证时一次登录即可）。
func newAdminClient(t *testing.T, env *testsupport.Env) *testsupport.Client {
	t.Helper()

	env.CreateAdmin(adminEmail, adminPassword)
	return loginAdmin(t, env, adminEmail, adminPassword)
}

// newSupportClient 创建 support 角色管理员并登录，返回会话与管理员 ID，
// 用于验证 support 全线只读。
func newSupportClient(t *testing.T, env *testsupport.Env) (*testsupport.Client, int64) {
	t.Helper()

	id := env.CreateAdminWithRole(supportEmail, adminPassword, "support")
	return loginAdmin(t, env, supportEmail, adminPassword), id
}

// newOpsClient 创建 ops 角色管理员并登录。ops 与 owner 目前能力相同。
func newOpsClient(t *testing.T, env *testsupport.Env) *testsupport.Client {
	t.Helper()

	env.CreateAdminWithRole(opsEmail, adminPassword, "ops")
	return loginAdmin(t, env, opsEmail, adminPassword)
}

// paidOrder 下一笔单并用回调把它置为已支付，供退款用例使用。
//
// 新订单不再经 HTTP 创建（充值已跳 Sub2API），这里从服务层注入。
func paidOrder(t *testing.T, env *testsupport.Env, userID int64) string {
	t.Helper()

	orderNo := env.Checkout(userID, "mock")
	payload, signature := notify(t, env, orderNo, true, 6800)
	env.NewClient().PostRaw("/api/v1/billing/webhook/mock", payload,
		map[string]string{"X-CCHaven-Signature": signature}).ExpectStatus(http.StatusOK)
	return orderNo
}

func loginAdmin(t *testing.T, env *testsupport.Env, email, password string) *testsupport.Client {
	t.Helper()

	client := env.NewClient()
	client.Post("/api/admin/v1/auth/login", map[string]string{
		"email": email, "password": password,
	}).ExpectStatus(http.StatusOK)
	return client
}

// TestAdminIsSeparateFromUserAccounts 验证管理员与普通用户是两套完全隔离的体系。
func TestAdminIsSeparateFromUserAccounts(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")

	// 普通用户会话访问管理端应被拒。
	browser.Get("/api/admin/v1/metrics/overview").ExpectStatus(http.StatusUnauthorized)

	// 管理员账号也不能用来登录用户侧：本地登录已收口到 Lumio 账号中心，
	// 该端点对任何人（包括管理员）恒 410，隔离语义不变。
	env.CreateAdmin(adminEmail, adminPassword)
	env.NewClient().Post("/api/v1/auth/login", map[string]string{
		"email": adminEmail, "password": adminPassword,
	}).ExpectStatus(http.StatusGone)
}

func TestAdminLoginRejectsWrongPassword(t *testing.T) {
	env := testsupport.New(t)
	env.CreateAdmin(adminEmail, adminPassword)

	resp := env.NewClient().Post("/api/admin/v1/auth/login", map[string]string{
		"email": adminEmail, "password": "WrongPass1",
	}).ExpectStatus(http.StatusUnauthorized)

	if got := resp.ErrorMessage(); got != "邮箱或密码不正确。" {
		t.Errorf("文案 = %q", got)
	}
}

// TestAdminWriteFromAdminOrigin 验证后台自己的来源能完成写操作。
//
// 后台独立部署在 admin.cchaven.cn，与官网不同源；可信来源里漏掉它时，
// 生产环境下禁用用户、退款、改运营配置会全部 403，而 dev 放行 localhost 看不出来。
func TestAdminWriteFromAdminOrigin(t *testing.T) {
	env := testsupport.New(t)
	_, userID := env.SignUp("alice@example.com")

	admin := newAdminClient(t, env)

	admin.WithHeader("Origin", env.Cfg.AdminURL).
		Post(disablePath(userID), map[string]string{"reason": "滥用"}).
		ExpectStatus(http.StatusOK)

	// 同一个会话，换成第三方站点的来源，写操作仍然要被挡下。
	admin.WithHeader("Origin", "https://evil.example.com").
		Post(enablePath(userID), nil).
		ExpectStatus(http.StatusForbidden)
}

// TestAdminTOTPEnrollmentAndGate 验证两步验证注册流程，
// 以及启用后未通过 TOTP 的半会话无法访问业务接口。
func TestAdminTOTPEnrollmentAndGate(t *testing.T) {
	env := testsupport.New(t)
	client := newAdminClient(t, env)

	// 注册 TOTP。
	setup := client.Post("/api/admin/v1/auth/totp/setup", nil).ExpectStatus(http.StatusOK)
	secret := setup.String("secret")
	if secret == "" {
		t.Fatal("应返回 TOTP 种子")
	}
	if !strings.Contains(setup.String("uri"), "CCHaven%20Admin") {
		t.Errorf("otpauth URI 应包含发行方, got %q", setup.String("uri"))
	}

	code, err := totp.GenerateCode(secret, env.Now())
	if err != nil {
		t.Fatalf("生成验证码失败: %v", err)
	}
	client.Post("/api/admin/v1/auth/totp/enable", map[string]string{"code": code}).
		ExpectStatus(http.StatusOK)

	// 种子应加密存储，数据库里看不到明文。
	var stored string
	if err := env.Pool.QueryRow(t.Context(),
		`SELECT totp_secret_enc FROM admins WHERE email = $1`, adminEmail).Scan(&stored); err != nil {
		t.Fatalf("查询 TOTP 种子失败: %v", err)
	}
	if strings.Contains(stored, secret) {
		t.Error("TOTP 种子不应以明文存储")
	}

	// 重新登录：此时只拿到半会话。
	next := env.NewClient()
	login := next.Post("/api/admin/v1/auth/login", map[string]string{
		"email": adminEmail, "password": adminPassword,
	}).ExpectStatus(http.StatusOK)
	if login.Data()["mfa_required"] != true {
		t.Fatal("启用 TOTP 后登录应要求两步验证")
	}

	// 半会话访问业务接口被拒。
	gated := next.Get("/api/admin/v1/metrics/overview").ExpectStatus(http.StatusUnauthorized)
	if gated.ErrorCode() != "mfa_required" {
		t.Errorf("错误码 = %q, want mfa_required", gated.ErrorCode())
	}

	// 错误验证码不放行。
	next.Post("/api/admin/v1/auth/login/totp", map[string]string{"code": "000000"}).
		ExpectStatus(http.StatusUnauthorized)

	// 正确验证码升级会话。
	valid, _ := totp.GenerateCode(secret, env.Now())
	next.Post("/api/admin/v1/auth/login/totp", map[string]string{"code": valid}).
		ExpectStatus(http.StatusOK)
	next.Get("/api/admin/v1/metrics/overview").ExpectStatus(http.StatusOK)
}

// TestAdminTOTPBruteForceLocksAccount 锁住 QA S-1：TOTP 验证码错误必须计入
// 按账号的失败锁定。TOTP 只有 10^6 种取值，拿到口令的攻击者若能无限重试，
// 小时级在线穷举就能接管后台——错误计数与口令登录共用 5 次锁 15 分钟。
func TestAdminTOTPBruteForceLocksAccount(t *testing.T) {
	env := testsupport.New(t)
	client := newAdminClient(t, env)

	setup := client.Post("/api/admin/v1/auth/totp/setup", nil).ExpectStatus(http.StatusOK)
	secret := setup.String("secret")
	code, err := totp.GenerateCode(secret, env.Now())
	if err != nil {
		t.Fatalf("生成验证码失败: %v", err)
	}
	client.Post("/api/admin/v1/auth/totp/enable", map[string]string{"code": code}).
		ExpectStatus(http.StatusOK)

	next := env.NewClient()
	next.Post("/api/admin/v1/auth/login", map[string]string{
		"email": adminEmail, "password": adminPassword,
	}).ExpectStatus(http.StatusOK)

	// 连续错误触发锁定；次数刻意引用口令锁定的阈值常量，两处此后不会漂移。
	for i := 0; i < service.AdminLoginFailureThreshold; i++ {
		next.Post("/api/admin/v1/auth/login/totp", map[string]string{"code": "000000"}).
			ExpectStatus(http.StatusUnauthorized)
	}

	// 锁定期间，正确的验证码也不得放行。
	valid, _ := totp.GenerateCode(secret, env.Now())
	locked := next.Post("/api/admin/v1/auth/login/totp", map[string]string{"code": valid}).
		ExpectStatus(http.StatusLocked)
	if locked.ErrorCode() != "account_locked" {
		t.Errorf("错误码 = %q, want account_locked", locked.ErrorCode())
	}
}

// TestAdminDisableUserLogsOutImmediately 验证禁用用户「立即被登出且无法登录」，并留审计。
func TestAdminDisableUserLogsOutImmediately(t *testing.T) {
	env := testsupport.New(t)
	browser, userID := env.SignUp("alice@example.com")
	session := env.AuthorizeApp(browser, "device-1")
	appClient := env.NewClient().WithBearer(session.AccessToken)

	admin := newAdminClient(t, env)
	admin.Post(disablePath(userID), map[string]string{"reason": "滥用"}).
		ExpectStatus(http.StatusOK)

	// 浏览器与 APP 会话都立即失效：浏览器走 Sub2API 令牌路径，本地禁用
	// 对它返回明确的「账号已停用」；APP 的本地会话族被撤销，表现为会话过期。
	browser.Get("/api/v1/me").ExpectStatus(http.StatusForbidden)
	appClient.Get("/api/v1/me").ExpectStatus(http.StatusUnauthorized)

	// 审计日志记录了操作人与前后值。
	logs := admin.Get("/api/admin/v1/audit-logs").ExpectStatus(http.StatusOK).Array("items")
	if len(logs) == 0 {
		t.Fatal("禁用操作应留审计日志")
	}
	entry := logs[0].(map[string]any)
	if entry["action"] != "user.disable" {
		t.Errorf("审计动作 = %v", entry["action"])
	}
	if entry["before"] == nil || entry["after"] == nil {
		t.Errorf("审计应记录前后值: %v", entry)
	}

	// 解禁后恢复：浏览器重新以 Sub2API 令牌访问即正常（本地登录已收口，
	// 不再有「重新登录」端点可断言）。
	admin.Post(enablePath(userID), nil).ExpectStatus(http.StatusOK)
	browser.Get("/api/v1/me").ExpectStatus(http.StatusOK)
}

// TestAdminUserListMasksEmailAndFilters 验证列表打码与筛选 chips。
func TestAdminUserListMasksEmailAndFilters(t *testing.T) {
	env := testsupport.New(t)

	trialBrowser, _ := env.SignUp("trial@example.com")
	env.AuthorizeApp(trialBrowser, "device-trial")
	env.SignUp("plain@example.com")

	admin := newAdminClient(t, env)

	all := admin.Get("/api/admin/v1/users?status=all").ExpectStatus(http.StatusOK)
	if got := all.Number("total"); got != 2 {
		t.Errorf("用户总数 = %v, want 2", got)
	}
	for _, raw := range all.Array("items") {
		item := raw.(map[string]any)
		email := item["email_masked"].(string)
		if !strings.Contains(email, "***") {
			t.Errorf("列表邮箱应打码, got %q", email)
		}
	}

	trialOnly := admin.Get("/api/admin/v1/users?status=trial").ExpectStatus(http.StatusOK)
	if got := trialOnly.Number("total"); got != 1 {
		t.Errorf("试用中用户数 = %v, want 1", got)
	}

	noneOnly := admin.Get("/api/admin/v1/users?status=none").ExpectStatus(http.StatusOK)
	if got := noneOnly.Number("total"); got != 1 {
		t.Errorf("未订阅用户数 = %v, want 1", got)
	}

	search := admin.Get("/api/admin/v1/users?query=plain").ExpectStatus(http.StatusOK)
	if got := search.Number("total"); got != 1 {
		t.Errorf("搜索结果数 = %v, want 1", got)
	}

	empty := admin.Get("/api/admin/v1/users?query=nobody").ExpectStatus(http.StatusOK)
	if got := empty.Number("total"); got != 0 {
		t.Errorf("无匹配时应返回 0, got %v", got)
	}
}

// TestAdminUserListShowsPlatformAndSource 验证「使用平台」与「来源」两列。
func TestAdminUserListShowsPlatformAndSource(t *testing.T) {
	env := testsupport.New(t)

	inviterBrowser, inviterID := env.SignUp("alice@example.com")
	code := env.ReferralCodeOf(inviterID)

	inviteeBrowser := env.NewClient()
	inviteeBrowser.Get("/api/v1/invites/" + code).ExpectStatus(http.StatusOK)
	inviteeBrowser, _ = env.Identify(inviteeBrowser, "bob@example.com")
	session := env.AuthorizeApp(inviteeBrowser, "device-bob")

	// 心跳补全设备信息后，列表才有「使用平台」。
	env.NewClient().WithBearer(session.AccessToken).
		Post("/api/v1/app/heartbeat", map[string]string{
			"device_id": "device-bob", "app_version": "1.4.2",
			"os_version": "15", "arch": "arm64",
		}).ExpectStatus(http.StatusOK)

	_ = inviterBrowser
	admin := newAdminClient(t, env)
	items := admin.Get("/api/admin/v1/users?query=bob").ExpectStatus(http.StatusOK).Array("items")
	if len(items) != 1 {
		t.Fatalf("应查到 1 个用户, got %d", len(items))
	}

	item := items[0].(map[string]any)
	if got := item["platform"]; got != "macOS 15 · Apple Silicon" {
		t.Errorf("使用平台 = %v", got)
	}
	if got := item["source"]; got != "邀请" {
		t.Errorf("来源 = %v, want 邀请", got)
	}
	if got := item["inviter_id"]; got == nil || got == "" {
		t.Error("邀请来源应带上邀请者 ID")
	}
	if got := item["sub_state"]; got != "trial" {
		t.Errorf("订阅状态 = %v, want trial", got)
	}
}

// TestAdminUserDetailReturnsPlainEmailAndSnapshots 验证详情页拿到明文邮箱与四块附属数据，
// 且每次访问都在审计日志里留痕。
func TestAdminUserDetailReturnsPlainEmailAndSnapshots(t *testing.T) {
	env := testsupport.New(t)

	inviterBrowser, inviterID := env.SignUp("alice@example.com")
	session := env.AuthorizeApp(inviterBrowser, "device-alice")
	env.NewClient().WithBearer(session.AccessToken).
		Post("/api/v1/app/heartbeat", map[string]string{
			"device_id": "device-alice", "app_version": "1.4.2",
			"os_version": "15", "arch": "arm64",
		}).ExpectStatus(http.StatusOK)

	// 被邀请者走完闭环，邀请者才有邀请进度与奖励天数。
	code := env.ReferralCodeOf(inviterID)
	invitee := env.NewClient()
	invitee.Get("/api/v1/invites/" + code).ExpectStatus(http.StatusOK)
	invitee, _ = env.Identify(invitee, "bob@example.com")
	env.AuthorizeApp(invitee, "device-bob")

	orderNo := env.Checkout(inviterID, "mock")

	admin := newAdminClient(t, env)
	detail := admin.Get(userPath(inviterID)).ExpectStatus(http.StatusOK)

	user := detail.Object("user")
	if got := user["email"]; got != "alice@example.com" {
		t.Errorf("详情页邮箱应为明文, got %v", got)
	}
	if got := user["id"]; got != "U-"+strconv.FormatInt(inviterID, 10) {
		t.Errorf("展示号 = %v", got)
	}
	if got := user["user_id"]; got != float64(inviterID) {
		t.Errorf("详情 user_id = %v, want %d", got, inviterID)
	}
	if got := user["status"]; got != "active" {
		t.Errorf("账号状态 = %v, want active", got)
	}
	if got := user["source"]; got != "自然流量" {
		t.Errorf("注册来源 = %v", got)
	}

	entitlement := detail.Object("entitlement")
	// 邀请闭环已把 7 天奖励按付费天数入账（KindPaid），试用订阅随之升级：
	// 状态是 active 而非 trialing（applyDays 的 kind 改写规则）。
	if got := entitlement["status"]; got != "active" {
		t.Errorf("订阅状态 = %v, want active", got)
	}
	if got := entitlement["bonus_days_total"]; got != float64(7) {
		t.Errorf("累计奖励天数 = %v, want 7", got)
	}

	devices := detail.Array("devices")
	if len(devices) != 1 {
		t.Fatalf("设备数 = %d, want 1", len(devices))
	}
	if got := devices[0].(map[string]any)["platform"]; got != "macOS 15 · Apple Silicon" {
		t.Errorf("设备平台 = %v", got)
	}

	referral := detail.Object("referral")
	if got := referral["invited_count"]; got != float64(1) {
		t.Errorf("已邀请人数 = %v, want 1", got)
	}
	if got := referral["total_bonus_days"]; got != float64(7) {
		t.Errorf("邀请累计延长天数 = %v, want 7", got)
	}
	items, _ := referral["items"].([]any)
	if len(items) != 1 {
		t.Fatalf("邀请进度条目数 = %d, want 1", len(items))
	}
	// 被邀请者是另一个人，其邮箱仍需打码。
	if got := items[0].(map[string]any)["email_masked"]; got != "b***b@example.com" {
		t.Errorf("邀请进度邮箱 = %v, 应打码", got)
	}

	orders := detail.Array("orders")
	if len(orders) != 1 {
		t.Fatalf("订单数 = %d, want 1", len(orders))
	}
	if got := orders[0].(map[string]any)["order_no"]; got != orderNo {
		t.Errorf("订单号 = %v, want %s", got, orderNo)
	}

	// 查看明文邮箱是敏感操作，每次访问都要留痕。
	logs := admin.Get("/api/admin/v1/audit-logs?action=user.view_detail").
		ExpectStatus(http.StatusOK)
	if got := logs.Number("total"); got != 1 {
		t.Fatalf("user.view_detail 审计条数 = %v, want 1", got)
	}
	entry := logs.Array("items")[0].(map[string]any)
	if got := entry["target_id"]; got != strconv.FormatInt(inviterID, 10) {
		t.Errorf("审计目标 = %v, want %d", got, inviterID)
	}
	if got := entry["actor_type"]; got != "admin" {
		t.Errorf("审计操作人类型 = %v", got)
	}

	// 再看一次就再留一条，而不是去重。
	admin.Get(userPath(inviterID)).ExpectStatus(http.StatusOK)
	again := admin.Get("/api/admin/v1/audit-logs?action=user.view_detail").
		ExpectStatus(http.StatusOK)
	if got := again.Number("total"); got != 2 {
		t.Errorf("二次访问后审计条数 = %v, want 2", got)
	}
}

// TestAdminUserDetailNeedsElevatedRole 验证「详情页需二次权限」：
// support 角色被挡在外面，且这次越权尝试同样留痕。
func TestAdminUserDetailNeedsElevatedRole(t *testing.T) {
	env := testsupport.New(t)
	_, userID := env.SignUp("alice@example.com")

	support, supportID := newSupportClient(t, env)
	denied := support.Get(userPath(userID)).ExpectStatus(http.StatusForbidden)
	if denied.ErrorCode() != "forbidden" {
		t.Errorf("错误码 = %q, want forbidden", denied.ErrorCode())
	}

	// 列表仍然可用，只是邮箱打码——客服的日常工单不需要明文。
	items := support.Get("/api/admin/v1/users").ExpectStatus(http.StatusOK).Array("items")
	if len(items) != 1 {
		t.Fatalf("用户数 = %d, want 1", len(items))
	}
	row := items[0].(map[string]any)
	if got := row["email_masked"]; got != "a***e@example.com" {
		t.Errorf("列表邮箱 = %v, 应打码", got)
	}
	// 列表行直接带上调接口用的主键，前端不必从展示号 U-{id} 反解。
	if got := row["user_id"]; got != float64(userID) {
		t.Errorf("列表 user_id = %v, want %d", got, userID)
	}

	logs := support.Get("/api/admin/v1/audit-logs?action=user.view_detail_denied").
		ExpectStatus(http.StatusOK)
	if got := logs.Number("total"); got != 1 {
		t.Fatalf("越权审计条数 = %v, want 1", got)
	}
	entry := logs.Array("items")[0].(map[string]any)
	if got := entry["actor_id"]; got != strconv.FormatInt(supportID, 10) {
		t.Errorf("审计操作人 = %v, want %d", got, supportID)
	}
	if got := entry["target_id"]; got != strconv.FormatInt(userID, 10) {
		t.Errorf("审计目标 = %v, want %d", got, userID)
	}
	after, _ := entry["after"].(map[string]any)
	if after["actor_role"] != "support" {
		t.Errorf("审计应记录被拒角色, got %v", entry["after"])
	}

	// 被拒的访问不应产生成功查看的记录。
	success := support.Get("/api/admin/v1/audit-logs?action=user.view_detail").
		ExpectStatus(http.StatusOK)
	if got := success.Number("total"); got != 0 {
		t.Errorf("user.view_detail 条数 = %v, want 0", got)
	}
}

// TestAdminSupportCannotWrite 验证 support 是全线只读角色。
//
// 这是权限矩阵里最要紧的一条：破坏性操作的门槛不得低于读取敏感信息。
// 曾经的实现只挡住了「看明文邮箱」，却放行禁用用户、退款、改价格，
// 使一个连邮箱都看不到的账号可以把真实客户锁在门外。
func TestAdminSupportCannotWrite(t *testing.T) {
	env := testsupport.New(t)
	_, userID := env.SignUp("alice@example.com")
	orderNo := paidOrder(t, env, userID)

	support, supportID := newSupportClient(t, env)

	cases := []struct {
		name    string
		action  string
		target  string
		attempt func() *testsupport.Response
	}{
		{
			name:    "禁用用户",
			action:  "user.disable_denied",
			target:  strconv.FormatInt(userID, 10),
			attempt: func() *testsupport.Response { return support.Post(disablePath(userID), nil) },
		},
		{
			name:    "解禁用户",
			action:  "user.enable_denied",
			target:  strconv.FormatInt(userID, 10),
			attempt: func() *testsupport.Response { return support.Post(enablePath(userID), nil) },
		},
		{
			name:   "订单退款",
			action: "order.refund_denied",
			target: orderNo,
			attempt: func() *testsupport.Response {
				return support.Post("/api/admin/v1/orders/"+orderNo+"/refund", nil)
			},
		},
		{
			name:   "修改运营配置",
			action: "ops_config.update_denied",
			// 目标是本次提交的 key 列表（已排序），回溯时能看出他想改哪几项。
			target: "invite.reward_days,pricing.monthly",
			attempt: func() *testsupport.Response {
				return support.Put("/api/admin/v1/configs", map[string]any{
					"pricing.monthly":    map[string]any{"amount_cents": 100, "currency": "CNY"},
					"invite.reward_days": 999,
				})
			},
		},
		{
			name:   "导出订单 CSV",
			action: "orders.export_denied",
			// 导出的目标不是某一笔订单，而是筛选条件；这里筛的是已支付。
			target:  "paid",
			attempt: func() *testsupport.Response { return support.Get("/api/admin/v1/orders/export?status=paid") },
		},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			denied := tc.attempt().ExpectStatus(http.StatusForbidden)
			if denied.ErrorCode() != "forbidden" {
				t.Errorf("错误码 = %q, want forbidden", denied.ErrorCode())
			}

			// 越权尝试必须留痕，且能按 `{原动作}_denied` 查到。
			logs := support.Get("/api/admin/v1/audit-logs?action=" + tc.action).
				ExpectStatus(http.StatusOK)
			if got := logs.Number("total"); got != 1 {
				t.Fatalf("%s 审计条数 = %v, want 1", tc.action, got)
			}
			entry := logs.Array("items")[0].(map[string]any)
			if got := entry["actor_id"]; got != strconv.FormatInt(supportID, 10) {
				t.Errorf("审计操作人 = %v, want %d", got, supportID)
			}
			if got := entry["target_id"]; got != tc.target {
				t.Errorf("审计目标 = %v, want %q", got, tc.target)
			}
			if got := entry["ip"]; got == "" {
				t.Error("审计应记录 IP")
			}
			after, _ := entry["after"].(map[string]any)
			if after["actor_role"] != "support" {
				t.Errorf("审计应记录被拒角色, got %v", entry["after"])
			}
		})
	}

	// 没有任何一次尝试真的生效：账号仍 active、订单仍是已支付、配置没被改。
	// 本地登录已收口到 Lumio 账号中心（该端点设计上恒 410），这里改从
	// 管理端确认账号状态，断言意图不变。
	owner := newAdminClient(t, env)
	detail := owner.Get(userPath(userID)).ExpectStatus(http.StatusOK)
	if got := detail.Object("user")["status"]; got != "active" {
		t.Errorf("用户状态 = %v, want active", got)
	}

	orders := support.Get("/api/admin/v1/orders?status=paid").ExpectStatus(http.StatusOK).Array("items")
	if len(orders) != 1 {
		t.Fatalf("已支付订单数 = %d, want 1", len(orders))
	}
	public := env.NewClient().Get("/api/v1/config/public").ExpectStatus(http.StatusOK)
	if got := public.Object("invite")["reward_days"]; got == float64(999) {
		t.Error("support 的越权提交不应改动运营配置")
	}

	// 只读能力不受影响：客服照常查指标、用户列表、订单列表与审计日志。
	support.Get("/api/admin/v1/metrics/overview").ExpectStatus(http.StatusOK)
	support.Get("/api/admin/v1/users").ExpectStatus(http.StatusOK)
	support.Get("/api/admin/v1/orders").ExpectStatus(http.StatusOK)
	support.Get("/api/admin/v1/configs").ExpectStatus(http.StatusOK)
	support.Get("/api/admin/v1/audit-logs").ExpectStatus(http.StatusOK)
}

// TestAdminOwnerAndOpsShareFullAccess 验证 owner 与 ops 在同样的操作上都放行。
// 两者当前没有能力差异，这个用例就是那句「暂无差异」的可执行版本。
func TestAdminOwnerAndOpsShareFullAccess(t *testing.T) {
	env := testsupport.New(t)
	_, userID := env.SignUp("alice@example.com")
	firstOrder := paidOrder(t, env, userID)
	secondOrder := paidOrder(t, env, userID)

	owner := newAdminClient(t, env)
	ops := newOpsClient(t, env)

	clients := []struct {
		role   string
		client *testsupport.Client
		order  string
	}{
		{"owner", owner, firstOrder},
		{"ops", ops, secondOrder},
	}
	for _, tc := range clients {
		t.Run(tc.role, func(t *testing.T) {
			tc.client.Get(userPath(userID)).ExpectStatus(http.StatusOK)
			tc.client.Post(disablePath(userID), map[string]string{"reason": "滥用"}).
				ExpectStatus(http.StatusOK)
			tc.client.Post(enablePath(userID), nil).ExpectStatus(http.StatusOK)
			tc.client.Post("/api/admin/v1/orders/"+tc.order+"/refund", nil).
				ExpectStatus(http.StatusOK)
			tc.client.Put("/api/admin/v1/configs", map[string]any{"invite.reward_days": 14}).
				ExpectStatus(http.StatusOK)

			export := tc.client.Get("/api/admin/v1/orders/export").ExpectStatus(http.StatusOK)
			if !strings.Contains(string(export.Raw), "订单号") {
				t.Errorf("导出内容不像 CSV: %q", export.Raw)
			}
		})
	}

	// 放行路径不写 `_denied`：审计里出现它就说明矩阵判反了。
	for _, action := range []string{
		"user.view_detail_denied", "user.disable_denied", "user.enable_denied",
		"order.refund_denied", "ops_config.update_denied", "orders.export_denied",
	} {
		logs := owner.Get("/api/admin/v1/audit-logs?action=" + action).ExpectStatus(http.StatusOK)
		if got := logs.Number("total"); got != 0 {
			t.Errorf("%s 条数 = %v, want 0", action, got)
		}
	}
}

// TestAdminSuccessfulExportIsAudited 验证成功的导出也留痕。
//
// 我们正是以「一次性把大量用户邮箱落到本地文件」为由把导出划进写操作的，
// 那么真正发生了外带的那一次比被拒的那一次更该可追溯。只审计失败是自相矛盾。
func TestAdminSuccessfulExportIsAudited(t *testing.T) {
	env := testsupport.New(t)
	_, userID := env.SignUp("alice@example.com")
	paidOrder(t, env, userID)

	admin := newAdminClient(t, env)
	admin.Get("/api/admin/v1/orders/export?status=paid").ExpectStatus(http.StatusOK)

	logs := admin.Get("/api/admin/v1/audit-logs?action=orders.export").ExpectStatus(http.StatusOK)
	if got := logs.Number("total"); got != 1 {
		t.Fatalf("成功导出应留下 1 条审计, got %v", got)
	}

	entry := logs.Array("items")[0].(map[string]any)
	after, ok := entry["after"].(map[string]any)
	if !ok {
		t.Fatalf("审计缺少 after: %v", entry)
	}
	// 留痕必须能回答「导走了什么范围、多少行」，否则事后无法评估外带影响面。
	if after["status_filter"] != "paid" {
		t.Errorf("筛选条件 = %v, want paid", after["status_filter"])
	}
	if after["row_count"] != float64(1) {
		t.Errorf("导出行数 = %v, want 1", after["row_count"])
	}
}

func TestAdminUserDetailNotFound(t *testing.T) {
	env := testsupport.New(t)
	admin := newAdminClient(t, env)

	missing := admin.Get(userPath(999999)).ExpectStatus(http.StatusNotFound)
	if missing.ErrorCode() != "not_found" {
		t.Errorf("错误码 = %q, want not_found", missing.ErrorCode())
	}
}

// TestAdminAuditLogFilters 验证按操作人、按动作以及两者组合筛选审计日志。
func TestAdminAuditLogFilters(t *testing.T) {
	env := testsupport.New(t)
	_, userID := env.SignUp("alice@example.com")

	ownerID := env.CreateAdmin(adminEmail, adminPassword)
	owner := loginAdmin(t, env, adminEmail, adminPassword)
	opsID := env.CreateAdminWithRole(opsEmail, adminPassword, "ops")
	ops := loginAdmin(t, env, opsEmail, adminPassword)

	owner.Post(disablePath(userID), map[string]string{"reason": "滥用"}).
		ExpectStatus(http.StatusOK)
	ops.Put("/api/admin/v1/configs", map[string]any{"invite.reward_days": 14}).
		ExpectStatus(http.StatusOK)

	// 不带筛选时行为不变：两条记录都在。
	if got := owner.Get("/api/admin/v1/audit-logs").ExpectStatus(http.StatusOK).Number("total"); got != 2 {
		t.Fatalf("审计总数 = %v, want 2", got)
	}

	ownerActor := strconv.FormatInt(ownerID, 10)
	opsActor := strconv.FormatInt(opsID, 10)

	cases := []struct {
		name  string
		query string
		want  float64
	}{
		{"按动作筛选", "?action=user.disable", 1},
		{"按操作人筛选", "?actor=" + ownerActor, 1},
		{"组合筛选命中", "?actor=" + opsActor + "&action=ops_config.update", 1},
		{"组合筛选不命中", "?actor=" + ownerActor + "&action=ops_config.update", 0},
		{"未知动作", "?action=user.view_detail", 0},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			resp := owner.Get("/api/admin/v1/audit-logs" + tc.query).ExpectStatus(http.StatusOK)
			if got := resp.Number("total"); got != tc.want {
				t.Errorf("total = %v, want %v", got, tc.want)
			}
			if got := len(resp.Array("items")); float64(got) != tc.want {
				t.Errorf("条目数 = %d, want %v", got, tc.want)
			}
		})
	}
}

// TestAdminMetricsOverview 验证六张指标卡，特别是缺数时返回 null 而非 0。
func TestAdminMetricsOverview(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")
	env.AuthorizeApp(browser, "device-1")

	admin := newAdminClient(t, env)
	overview := admin.Get("/api/admin/v1/metrics/overview").ExpectStatus(http.StatusOK)

	if got := overview.Object("dau")["value"]; got != float64(1) {
		t.Errorf("今日 DAU = %v, want 1", got)
	}
	if got := overview.Object("signups")["value"]; got != float64(1) {
		t.Errorf("今日新增注册 = %v, want 1", got)
	}
	if got := overview.Object("subscribers")["secondary"]; got != float64(1) {
		t.Errorf("试用中人数 = %v, want 1", got)
	}

	// 队列为空时留存返回 null，前端显示「—」而不是 0%。
	if got := overview.Object("retention_d7")["value"]; got != nil {
		t.Errorf("无队列时 7 日留存应为 null, got %v", got)
	}

	dau := admin.Get("/api/admin/v1/metrics/dau?days=7").ExpectStatus(http.StatusOK).Array("items")
	if len(dau) != 7 {
		t.Errorf("日活序列应补齐 7 天, got %d", len(dau))
	}
}

func TestAdminDistributions(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")
	session := env.AuthorizeApp(browser, "device-1")

	env.NewClient().WithBearer(session.AccessToken).
		Post("/api/v1/app/heartbeat", map[string]string{
			"device_id": "device-1", "app_version": "1.4.2",
			"os_version": "15", "arch": "arm64",
		}).ExpectStatus(http.StatusOK)

	admin := newAdminClient(t, env)
	resp := admin.Get("/api/admin/v1/metrics/distributions?days=30").ExpectStatus(http.StatusOK)

	platform := resp.Array("platform")
	if len(platform) != 1 || platform[0].(map[string]any)["label"] != "macOS · Apple Silicon" {
		t.Errorf("平台分布 = %v", platform)
	}

	versions := resp.Array("app_version")
	if len(versions) != 1 || versions[0].(map[string]any)["label"] != "1.4.2" {
		t.Errorf("版本分布 = %v", versions)
	}

	sources := resp.Array("source")
	if len(sources) != 1 || sources[0].(map[string]any)["label"] != "自然流量" {
		t.Errorf("来源分布 = %v", sources)
	}
}

// TestAdminOpsConfigUpdateIsAudited 验证运营配置改动实时生效且留前后值审计。
func TestAdminOpsConfigUpdateIsAudited(t *testing.T) {
	env := testsupport.New(t)
	admin := newAdminClient(t, env)

	put := admin.Put("/api/admin/v1/configs", map[string]any{
		"invite.reward_days": 14,
		"pricing.monthly":    map[string]any{"amount_cents": 9900, "currency": "CNY"},
	}).ExpectStatus(http.StatusOK)

	if got := put.Number("invite_reward_days"); got != 14 {
		t.Errorf("奖励天数 = %v, want 14", got)
	}

	// 官网侧立即读到新值。
	public := env.NewClient().Get("/api/v1/config/public").ExpectStatus(http.StatusOK)
	if got := public.Object("invite")["reward_days"]; got != float64(14) {
		t.Errorf("公开配置未实时生效, got %v", got)
	}
	if got := public.Object("pricing")["amount_cents"]; got != float64(9900) {
		t.Errorf("价格未实时生效, got %v", got)
	}

	logs := admin.Get("/api/admin/v1/audit-logs").ExpectStatus(http.StatusOK).Array("items")
	var found bool
	for _, raw := range logs {
		if entry := raw.(map[string]any); entry["action"] == "ops_config.update" {
			found = true
			if entry["after"] == nil {
				t.Error("配置审计应记录新值")
			}
		}
	}
	if !found {
		t.Error("配置改动应留审计日志")
	}
}

func TestAdminRefundRejectsUnpaidOrder(t *testing.T) {
	env := testsupport.New(t)
	_, userID := env.SignUp("alice@example.com")

	orderNo := env.Checkout(userID, "mock")

	admin := newAdminClient(t, env)
	resp := admin.Post("/api/admin/v1/orders/"+orderNo+"/refund", nil).
		ExpectStatus(http.StatusConflict)

	if resp.ErrorCode() != "order_not_refundable" {
		t.Errorf("错误码 = %q", resp.ErrorCode())
	}
}

func TestAdminOrderListIncludesTodaySummary(t *testing.T) {
	env := testsupport.New(t)
	_, userID := env.SignUp("alice@example.com")

	orderNo := env.Checkout(userID, "mock")
	payload, signature := notify(t, env, orderNo, true, 6800)
	env.NewClient().PostRaw("/api/v1/billing/webhook/mock", payload,
		map[string]string{"X-CCHaven-Signature": signature}).ExpectStatus(http.StatusOK)

	admin := newAdminClient(t, env)
	resp := admin.Get("/api/admin/v1/orders?status=paid").ExpectStatus(http.StatusOK)

	today := resp.Object("today")
	if today["count"] != float64(1) || today["amount_cents"] != float64(6800) {
		t.Errorf("当日汇总 = %v", today)
	}

	items := resp.Array("items")
	if len(items) != 1 {
		t.Fatalf("已支付订单数 = %d, want 1", len(items))
	}
	if got := items[0].(map[string]any)["email_masked"]; got != "a***e@example.com" {
		t.Errorf("订单列表邮箱应打码, got %v", got)
	}
}

func TestAdminSessionExpires(t *testing.T) {
	env := testsupport.New(t)
	admin := newAdminClient(t, env)

	admin.Get("/api/admin/v1/metrics/overview").ExpectStatus(http.StatusOK)

	env.Advance(env.Cfg.AdminSessionTTL + time.Minute)
	admin.Get("/api/admin/v1/metrics/overview").ExpectStatus(http.StatusUnauthorized)
}

func userPath(userID int64) string {
	return "/api/admin/v1/users/" + strconv.FormatInt(userID, 10)
}

func disablePath(userID int64) string { return userPath(userID) + "/disable" }
func enablePath(userID int64) string  { return userPath(userID) + "/enable" }
