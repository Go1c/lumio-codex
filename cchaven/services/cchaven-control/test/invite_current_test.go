package test

import (
	"net/http"
	"testing"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/testsupport"
)

// TestCurrentInviteReflectsCookie 验证首页邀请横幅的权威数据源：
// cch_ref 是 HttpOnly 的，前端读不到，只能由服务端回答「现在还带着有效邀请吗」。
func TestCurrentInviteReflectsCookie(t *testing.T) {
	env := testsupport.New(t)

	_, inviterID := env.SignUp("alice@example.com")
	code := env.ReferralCodeOf(inviterID)

	visitor := env.NewClient()

	// —— 没打开过邀请链接：不归因，也不许给出任何承诺 ——
	before := visitor.Get("/api/v1/invites/current").ExpectStatus(http.StatusOK)
	if before.Data()["attributed"] != false {
		t.Fatalf("无 cookie 时不应归因: %s", before.Raw)
	}
	if _, ok := before.Data()["inviter"]; ok {
		t.Errorf("未归因时不应下发邀请人: %s", before.Raw)
	}
	if _, ok := before.Data()["trial_days"]; ok {
		t.Errorf("未归因时不应下发试用天数: %s", before.Raw)
	}

	// —— 打开邀请链接拿到 cookie 之后：横幅有据可依 ——
	landing := visitor.Get("/api/v1/invites/" + code).ExpectStatus(http.StatusOK)

	current := visitor.Get("/api/v1/invites/current").ExpectStatus(http.StatusOK)
	if current.Data()["attributed"] != true {
		t.Fatalf("持有有效邀请 cookie 时应归因: %s", current.Raw)
	}
	// 字段语义必须与落地页一致，否则两个页面会讲出不一样的话。
	if got, want := current.String("inviter"), landing.String("inviter"); got != want {
		t.Errorf("邀请人不符: got %q want %q", got, want)
	}
	if got := current.Number("trial_days"); got != 30 {
		t.Errorf("试用天数不符: got %v want 30", got)
	}

	// —— 邀请码被停用：cookie 还在，但承诺立刻收回 ——
	if _, err := env.Pool.Exec(t.Context(),
		`UPDATE referral_codes SET disabled_at = now() WHERE code = $1`, code); err != nil {
		t.Fatalf("停用邀请码失败: %v", err)
	}

	after := visitor.Get("/api/v1/invites/current").ExpectStatus(http.StatusOK)
	if after.Data()["attributed"] != false {
		t.Fatalf("邀请码停用后不应再归因: %s", after.Raw)
	}
}

// TestCurrentInviteDoesNotRecordVisit 验证首页轮询不会污染邀请访问量。
//
// referral_visits 统计的是「邀请链接被打开」这一次事件，首页横幅每次渲染都会问一遍，
// 记进去会让三步闭环的第一步变成假数据。
func TestCurrentInviteDoesNotRecordVisit(t *testing.T) {
	env := testsupport.New(t)

	_, inviterID := env.SignUp("alice@example.com")
	code := env.ReferralCodeOf(inviterID)

	visitor := env.NewClient()
	visitor.Get("/api/v1/invites/" + code).ExpectStatus(http.StatusOK)

	if got := countReferralVisits(t, env, code); got != 1 {
		t.Fatalf("打开一次邀请链接应记 1 次访问, got %d", got)
	}

	for range 3 {
		visitor.Get("/api/v1/invites/current").ExpectStatus(http.StatusOK)
	}

	if got := countReferralVisits(t, env, code); got != 1 {
		t.Errorf("首页轮询不应新增访问记录, got %d want 1", got)
	}
}

// TestUnknownInviteCodeLeavesNoAttribution 验证打开一个不存在的邀请链接之后，
// 首页横幅不会被点亮：落地页既不下发 cookie 也不记访问。
func TestUnknownInviteCodeLeavesNoAttribution(t *testing.T) {
	env := testsupport.New(t)

	visitor := env.NewClient()
	landing := visitor.Get("/api/v1/invites/nosuchcode").ExpectStatus(http.StatusOK)
	if landing.Data()["valid"] != false {
		t.Fatalf("不存在的邀请码应为无效: %s", landing.Raw)
	}

	current := visitor.Get("/api/v1/invites/current").ExpectStatus(http.StatusOK)
	if current.Data()["attributed"] != false {
		t.Errorf("无效邀请码不应归因: %s", current.Raw)
	}
	if got := countReferralVisits(t, env, "nosuchcode"); got != 0 {
		t.Errorf("无效邀请码不应产生访问记录, got %d", got)
	}
}

func countReferralVisits(t *testing.T, env *testsupport.Env, code string) int {
	t.Helper()

	var visits int
	if err := env.Pool.QueryRow(t.Context(),
		`SELECT count(*) FROM referral_visits WHERE code = $1`, code).Scan(&visits); err != nil {
		t.Fatalf("查询访问记录失败: %v", err)
	}
	return visits
}
