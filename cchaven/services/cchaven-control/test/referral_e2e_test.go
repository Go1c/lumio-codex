package test

import (
	"net/http"
	"slices"
	"testing"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/store"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/testsupport"
)

// TestReferralClosureEndToEnd 覆盖 M1 的关键链路：
// 邀请链接访问 → 被邀请者首次带令牌到访（开户）→ 首次登录 APP → 试用发放 + 邀请者奖励。
//
// 这条链路横跨 identity、OAuth、订阅与邀请四个模块，是本里程碑的验收主线。
// 注册已经搬到 Lumio 账号中心，归因因此改在「影子账号被创建」那一刻结算——
// 那是本服务最后一次还能看到 cch_ref cookie 的机会。
func TestReferralClosureEndToEnd(t *testing.T) {
	env := testsupport.New(t)

	// —— 第 0 步：邀请者注册并拿到自己的邀请链接 ——
	inviterBrowser, inviterID := env.SignUp("alice@example.com")
	code := env.ReferralCodeOf(inviterID)

	referrals := inviterBrowser.Get("/api/v1/me/referrals").ExpectStatus(http.StatusOK)
	if got, want := referrals.String("link"), "https://cchaven.test/i/"+code; got != want {
		t.Errorf("邀请链接不符: got %q want %q", got, want)
	}
	if got := referrals.Number("reward_days"); got != 7 {
		t.Errorf("奖励天数应从运营配置读取，got %v want 7", got)
	}

	// —— 第 1 步：被邀请者打开邀请链接，服务端下发归因 cookie ——
	inviteeBrowser := env.NewClient()
	landing := inviteeBrowser.Get("/api/v1/invites/" + code).ExpectStatus(http.StatusOK)
	if landing.Data()["valid"] != true {
		t.Fatalf("邀请码应有效: %s", landing.Raw)
	}
	if got := landing.Number("trial_days"); got != 30 {
		t.Errorf("试用时长应为 30 天，got %v", got)
	}

	var visits int
	if err := env.Pool.QueryRow(t.Context(),
		`SELECT count(*) FROM referral_visits WHERE code = $1`, code).Scan(&visits); err != nil {
		t.Fatalf("查询访问记录失败: %v", err)
	}
	if visits != 1 {
		t.Errorf("应记录 1 次邀请链接访问，got %d", visits)
	}

	// —— 第 2 步：被邀请者在账号中心注册完回到 CC，开户（邀请码随 cookie 自动带入）——
	inviteeBrowser, inviteeID := env.Identify(inviteeBrowser, "bob@example.com")

	assertAttribution(t, env, inviteeID, "registered", inviterID)
	assertRegistrationSource(t, env, inviteeID, "invite")

	// 开户完成但尚未登录 APP，此时不得发放试用。
	if got := env.EntitlementOf(inviteeID)["status"]; got != "none" {
		t.Errorf("闭环未完成前不应有订阅，got %v", got)
	}

	// —— 第 3 步：首次登录 APP，触发发放 ——
	session := env.AuthorizeApp(inviteeBrowser, "device-bob-1")

	if session.Activation["trial_granted"] != true {
		t.Fatalf("首次登录 APP 应发放试用: %v", session.Activation)
	}
	if got := session.Activation["inviter_bonus_days"]; got != float64(7) {
		t.Errorf("邀请者奖励天数不符: got %v want 7", got)
	}

	// —— 断言：被邀请者获得 30 天试用 ——
	inviteeEntitlement := env.EntitlementOf(inviteeID)
	if got := inviteeEntitlement["status"]; got != "trialing" {
		t.Errorf("被邀请者应处于试用中，got %v", got)
	}
	if got := inviteeEntitlement["days_left"]; got != 30 {
		t.Errorf("试用剩余天数不符: got %v want 30", got)
	}

	// —— 断言：邀请者订阅延长 7 天 ——
	inviterEntitlement := env.EntitlementOf(inviterID)
	if got := inviterEntitlement["bonus_days_total"]; got != 7 {
		t.Errorf("邀请者累计延长天数不符: got %v want 7", got)
	}
	if got := inviterEntitlement["days_left"]; got != 7 {
		t.Errorf("邀请者剩余天数不符: got %v want 7", got)
	}

	// —— 断言：归因进入 activated，账户中心可见 ——
	assertAttribution(t, env, inviteeID, "activated", inviterID)

	progress := inviterBrowser.Get("/api/v1/me/referrals").ExpectStatus(http.StatusOK)
	if got := progress.Number("invited_count"); got != 1 {
		t.Errorf("已成功邀请人数不符: got %v want 1", got)
	}
	if got := progress.Number("total_bonus_days"); got != 7 {
		t.Errorf("累计延长天数不符: got %v want 7", got)
	}

	items := progress.Array("items")
	if len(items) != 1 {
		t.Fatalf("邀请进度列表应有 1 项，got %d", len(items))
	}
	item := items[0].(map[string]any)
	if item["status"] != "activated" {
		t.Errorf("邀请进度状态不符: got %v want activated", item["status"])
	}
	// 列表中的好友邮箱必须打码。
	if got := item["email_masked"]; got != "b***b@example.com" {
		t.Errorf("好友邮箱应打码: got %v", got)
	}

	// —— 断言：双方都收到通知邮件 ——
	if templates := env.OutboxTemplates("bob@example.com"); !slices.Contains(templates, store.TemplateTrialGranted) {
		t.Errorf("被邀请者应收到试用开通通知，实际模板 %v", templates)
	}
	if templates := env.OutboxTemplates("alice@example.com"); !slices.Contains(templates, store.TemplateInviteRewarded) {
		t.Errorf("邀请者应收到奖励到账通知，实际模板 %v", templates)
	}
}

// TestTrialGrantedOnlyOncePerAccount 验证「每个账号一生只可享用一次免费试用」。
func TestTrialGrantedOnlyOncePerAccount(t *testing.T) {
	env := testsupport.New(t)

	browser, userID := env.SignUp("carol@example.com")

	first := env.AuthorizeApp(browser, "device-carol-1")
	if first.Activation["trial_granted"] != true {
		t.Fatalf("首次登录 APP 应发放试用: %v", first.Activation)
	}

	daysAfterFirst := env.EntitlementOf(userID)["days_left"]

	// 第二次授权（换一台设备）不应再次发放。
	second := env.AuthorizeApp(browser, "device-carol-2")
	if second.Activation != nil && second.Activation["trial_granted"] == true {
		t.Errorf("同一账号不应二次发放试用: %v", second.Activation)
	}
	if got := env.EntitlementOf(userID)["days_left"]; got != daysAfterFirst {
		t.Errorf("二次授权不应改变订阅时长: got %v want %v", got, daysAfterFirst)
	}

	var grants int
	if err := env.Pool.QueryRow(t.Context(),
		`SELECT count(*) FROM subscription_events WHERE user_id = $1 AND type = 'trial_granted'`,
		userID).Scan(&grants); err != nil {
		t.Fatalf("查询发放事件失败: %v", err)
	}
	if grants != 1 {
		t.Errorf("试用发放事件应恰好 1 条，got %d", grants)
	}
}

// TestTrialDeniedForReusedDeviceFingerprint 验证同设备换账号重复领取会被拒绝，
// 且返回 6.2 节的固定文案。
func TestTrialDeniedForReusedDeviceFingerprint(t *testing.T) {
	env := testsupport.New(t)

	firstBrowser, _ := env.SignUp("dave@example.com")
	if got := env.AuthorizeApp(firstBrowser, "shared-device").Activation["trial_granted"]; got != true {
		t.Fatalf("首个账号应获得试用，got %v", got)
	}

	secondBrowser, secondID := env.SignUp("erin@example.com")
	activation := env.AuthorizeApp(secondBrowser, "shared-device").Activation

	if activation["trial_granted"] == true {
		t.Errorf("同一设备指纹不应重复发放试用: %v", activation)
	}
	if activation["trial_denied_reuse"] != true {
		t.Errorf("应标记为重复领取: %v", activation)
	}
	if got := env.EntitlementOf(secondID)["status"]; got != "none" {
		t.Errorf("被拒账号不应有订阅，got %v", got)
	}
}

// TestInviterRewardDisabledWhenConfiguredZero 验证奖励天数配为 0 时闭环仍完成、但不发奖励。
func TestInviterRewardDisabledWhenConfiguredZero(t *testing.T) {
	env := testsupport.New(t)
	env.SetOpsConfig("invite.reward_days", "0")

	inviterBrowser, inviterID := env.SignUp("frank@example.com")
	code := env.ReferralCodeOf(inviterID)

	inviteeBrowser := env.NewClient()
	inviteeBrowser.Get("/api/v1/invites/" + code).ExpectStatus(http.StatusOK)

	inviteeBrowser, inviteeID := env.Identify(inviteeBrowser, "grace@example.com")

	activation := env.AuthorizeApp(inviteeBrowser, "device-grace").Activation

	// 被邀请者照常拿到试用，闭环状态照常推进。
	if activation["trial_granted"] != true {
		t.Errorf("关闭邀请者奖励不应影响被邀请者试用: %v", activation)
	}
	if got := env.EntitlementOf(inviteeID)["status"]; got != "trialing" {
		t.Errorf("被邀请者应处于试用中，got %v", got)
	}
	assertAttribution(t, env, inviteeID, "activated", inviterID)

	// 邀请者不获得任何奖励。
	if got := env.EntitlementOf(inviterID)["status"]; got != "none" {
		t.Errorf("奖励关闭时邀请者不应获得订阅，got %v", got)
	}
	if got := env.EntitlementOf(inviterID)["bonus_days_total"]; got != 0 {
		t.Errorf("奖励关闭时累计延长天数应为 0，got %v", got)
	}

	overview := inviterBrowser.Get("/api/v1/me/referrals").ExpectStatus(http.StatusOK)
	if got := overview.Number("reward_days"); got != 0 {
		t.Errorf("接口应下发 reward_days=0 供前端隐藏文案，got %v", got)
	}
}

// TestSelfInviteIsRejected 验证不能用自己的邀请链接给自己发奖励。
func TestSelfInviteIsRejected(t *testing.T) {
	env := testsupport.New(t)

	browser, userID := env.SignUp("henry@example.com")
	code := env.ReferralCodeOf(userID)

	// 同一个浏览器带着自己的邀请 cookie 再注册一个账号是可以的，
	// 但邀请者与被邀请者必须是不同的人，数据库层的 CHECK 也会兜底。
	browser.Get("/api/v1/invites/" + code).ExpectStatus(http.StatusOK)

	var attributions int
	if err := env.Pool.QueryRow(t.Context(),
		`SELECT count(*) FROM referral_attributions WHERE inviter_user_id = invitee_user_id`).
		Scan(&attributions); err != nil {
		t.Fatalf("查询归因失败: %v", err)
	}
	if attributions != 0 {
		t.Errorf("不应存在自邀归因记录，got %d", attributions)
	}
}

func assertAttribution(t *testing.T, env *testsupport.Env, inviteeID int64, wantStage string, wantInviter int64) {
	t.Helper()

	var stage string
	var inviter int64
	err := env.Pool.QueryRow(t.Context(),
		`SELECT stage, inviter_user_id FROM referral_attributions WHERE invitee_user_id = $1`,
		inviteeID).Scan(&stage, &inviter)
	if err != nil {
		t.Fatalf("查询归因记录失败: %v", err)
	}
	if stage != wantStage {
		t.Errorf("归因阶段不符: got %q want %q", stage, wantStage)
	}
	if inviter != wantInviter {
		t.Errorf("邀请者不符: got %d want %d", inviter, wantInviter)
	}
}

func assertRegistrationSource(t *testing.T, env *testsupport.Env, userID int64, want string) {
	t.Helper()

	var source string
	if err := env.Pool.QueryRow(t.Context(),
		`SELECT registration_source FROM users WHERE id = $1`, userID).Scan(&source); err != nil {
		t.Fatalf("查询注册来源失败: %v", err)
	}
	if source != want {
		t.Errorf("注册来源不符: got %q want %q", source, want)
	}
}
