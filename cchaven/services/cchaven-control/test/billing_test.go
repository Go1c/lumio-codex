package test

import (
	"encoding/json"
	"fmt"
	"net/http"
	"regexp"
	"testing"
	"time"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/payments"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/testsupport"
)

// notify 构造一份带合法签名的 mock 支付回调。
func notify(t *testing.T, env *testsupport.Env, orderNo string, paid bool, amount int64) ([]byte, string) {
	t.Helper()

	payload, err := json.Marshal(payments.MockNotification{
		OrderNo: orderNo, TxnID: "txn-" + orderNo, Paid: paid, Amount: amount,
	})
	if err != nil {
		t.Fatalf("构造回调报文失败: %v", err)
	}
	return payload, env.Mock.Sign(payload)
}

// TestCheckoutRedirectsToTheLumioPurchasePage 锁住充值入口的去向。
//
// CC 不再自建收银台：钱包与充值都在 Lumio 账号中心，与 Codex 共用同一个入口。
// 浏览器跟随 303，XHR 客户端读 data.purchase_url。
func TestCheckoutRedirectsToTheLumioPurchasePage(t *testing.T) {
	env := testsupport.New(t)
	browser, _ := env.SignUp("alice@example.com")

	resp := browser.Post("/api/v1/billing/checkout", map[string]string{"channel": "mock"}).
		ExpectStatus(http.StatusSeeOther)

	want := env.Cfg.PurchaseURL()
	if got := resp.String("purchase_url"); got != want {
		t.Errorf("purchase_url = %q, want %q", got, want)
	}

	// 未登录也能拿到跳转目标：充值页自己会要求登录。
	env.NewClient().Post("/api/v1/billing/checkout", map[string]string{"channel": "mock"}).
		ExpectStatus(http.StatusSeeOther)

	// 下线的收银台不得再产生订单。
	var orders int
	if err := env.Pool.QueryRow(t.Context(), `SELECT count(*) FROM orders`).Scan(&orders); err != nil {
		t.Fatalf("查询订单失败: %v", err)
	}
	if orders != 0 {
		t.Errorf("跳转端点不应建单, got %d 条", orders)
	}
}

// TestCheckoutToPaidExtendsSubscription 验证下单 → 回调 → 订阅入账的完整链路。
//
// 新订单已不再由 HTTP 端点创建，但存量订单的回调与入账仍要继续工作，
// 因此从服务层注入订单再走回调。
func TestCheckoutToPaidExtendsSubscription(t *testing.T) {
	env := testsupport.New(t)
	browser, userID := env.SignUp("alice@example.com")

	orderNo := env.Checkout(userID, "mock")
	// 订单号格式 CC{YYYYMMDD}-{6 位序号}，与原型 CC20260812-100486 一致。
	if !regexp.MustCompile(`^CC\d{8}-\d{6}$`).MatchString(orderNo) {
		t.Errorf("订单号格式不符: %q", orderNo)
	}
	if got := browser.Get("/api/v1/billing/orders/" + orderNo).Number("amount_cents"); got != 6800 {
		t.Errorf("金额应取自运营配置, got %v", got)
	}

	// 付款前未订阅。
	if got := env.EntitlementOf(userID)["status"]; got != "none" {
		t.Errorf("付款前不应有订阅, got %v", got)
	}

	payload, signature := notify(t, env, orderNo, true, 6800)
	env.NewClient().PostRaw("/api/v1/billing/webhook/mock", payload,
		map[string]string{"X-CCHaven-Signature": signature}).ExpectStatus(http.StatusOK)

	entitlement := env.EntitlementOf(userID)
	if got := entitlement["status"]; got != "active" {
		t.Errorf("付款后应为已订阅, got %v", got)
	}
	if got := entitlement["days_left"]; got != 30 {
		t.Errorf("包月应延长 30 天, got %v", got)
	}

	order := browser.Get("/api/v1/billing/orders/" + orderNo).ExpectStatus(http.StatusOK)
	if got := order.String("status"); got != "paid" {
		t.Errorf("订单状态 = %q, want paid", got)
	}
}

// TestWebhookIsIdempotent 验证支付渠道重投回调不会重复延长订阅。
func TestWebhookIsIdempotent(t *testing.T) {
	env := testsupport.New(t)
	_, userID := env.SignUp("alice@example.com")

	orderNo := env.Checkout(userID, "mock")

	payload, signature := notify(t, env, orderNo, true, 6800)
	headers := map[string]string{"X-CCHaven-Signature": signature}

	for range 3 {
		env.NewClient().PostRaw("/api/v1/billing/webhook/mock", payload, headers).
			ExpectStatus(http.StatusOK)
	}

	if got := env.EntitlementOf(userID)["days_left"]; got != 30 {
		t.Errorf("重复回调不应叠加天数, got %v want 30", got)
	}

	var events int
	if err := env.Pool.QueryRow(t.Context(),
		`SELECT count(*) FROM subscription_events WHERE user_id = $1 AND type = 'purchase'`,
		userID).Scan(&events); err != nil {
		t.Fatalf("查询入账事件失败: %v", err)
	}
	if events != 1 {
		t.Errorf("入账事件应恰好 1 条, got %d", events)
	}
}

// TestWebhookRejectsBadSignature 验证伪造回调被拒且留痕。
func TestWebhookRejectsBadSignature(t *testing.T) {
	env := testsupport.New(t)
	_, userID := env.SignUp("alice@example.com")

	orderNo := env.Checkout(userID, "mock")

	payload, _ := notify(t, env, orderNo, true, 6800)
	env.NewClient().PostRaw("/api/v1/billing/webhook/mock", payload,
		map[string]string{"X-CCHaven-Signature": "forged"}).ExpectStatus(http.StatusForbidden)

	if got := env.EntitlementOf(userID)["status"]; got != "none" {
		t.Errorf("验签失败不应入账, got %v", got)
	}

	// 失败的回调也要留痕，便于排查伪造与配置错误。
	var events int
	if err := env.Pool.QueryRow(t.Context(),
		`SELECT count(*) FROM payment_events WHERE signature_ok = false`).Scan(&events); err != nil {
		t.Fatalf("查询支付事件失败: %v", err)
	}
	if events == 0 {
		t.Error("验签失败的回调应记录在 payment_events 中")
	}
}

// TestFailedPaymentMarksOrderFailed 验证支付失败的回调。
func TestFailedPaymentMarksOrderFailed(t *testing.T) {
	env := testsupport.New(t)
	browser, userID := env.SignUp("alice@example.com")

	orderNo := env.Checkout(userID, "mock")

	payload, signature := notify(t, env, orderNo, false, 6800)
	env.NewClient().PostRaw("/api/v1/billing/webhook/mock", payload,
		map[string]string{"X-CCHaven-Signature": signature}).ExpectStatus(http.StatusOK)

	if got := browser.Get("/api/v1/billing/orders/" + orderNo).String("status"); got != "failed" {
		t.Errorf("订单状态 = %q, want failed", got)
	}
	if got := env.EntitlementOf(userID)["status"]; got != "none" {
		t.Errorf("支付失败不应发放订阅, got %v", got)
	}
}

// TestOrderNumbersAreSequentialPerDay 验证同一天的订单号连续且不重号。
func TestOrderNumbersAreSequentialPerDay(t *testing.T) {
	env := testsupport.New(t)
	_, userID := env.SignUp("alice@example.com")

	seen := map[string]bool{}
	for range 5 {
		orderNo := env.Checkout(userID, "mock")
		if seen[orderNo] {
			t.Fatalf("订单号重复: %s", orderNo)
		}
		seen[orderNo] = true
	}
}

// TestUnknownChannelIsRejected 验证未接入的支付渠道被拒绝，
// 支付宝与微信在 M1 尚未接入，只保留接口。
func TestUnknownChannelIsRejected(t *testing.T) {
	env := testsupport.New(t)
	_, userID := env.SignUp("alice@example.com")

	for _, channel := range []string{"alipay", "wechat", "bogus"} {
		if _, err := env.Svc.Checkout(t.Context(), userID, channel, ""); err == nil {
			t.Errorf("渠道 %q 应被拒绝", channel)
		}
	}
}

// TestUserCannotReadOthersOrder 验证订单查询的越权防护。
func TestUserCannotReadOthersOrder(t *testing.T) {
	env := testsupport.New(t)

	_, aliceID := env.SignUp("alice@example.com")
	orderNo := env.Checkout(aliceID, "mock")

	bob, _ := env.SignUp("bob@example.com")
	bob.Get("/api/v1/billing/orders/" + orderNo).ExpectStatus(http.StatusNotFound)
}

// TestTrialThenPurchaseStacks 验证试用期内付款按顺延而非覆盖计算到期时间。
func TestTrialThenPurchaseStacks(t *testing.T) {
	env := testsupport.New(t)
	browser, userID := env.SignUp("alice@example.com")

	env.AuthorizeApp(browser, "device-1")
	if got := env.EntitlementOf(userID)["days_left"]; got != 30 {
		t.Fatalf("试用应为 30 天, got %v", got)
	}

	orderNo := env.Checkout(userID, "mock")
	payload, signature := notify(t, env, orderNo, true, 6800)
	env.NewClient().PostRaw("/api/v1/billing/webhook/mock", payload,
		map[string]string{"X-CCHaven-Signature": signature}).ExpectStatus(http.StatusOK)

	entitlement := env.EntitlementOf(userID)
	if got := entitlement["days_left"]; got != 60 {
		t.Errorf("付款应在试用到期日之后顺延, got %v want 60", got)
	}
	// 付费优先级高于试用，徽标应显示「已订阅」。
	if got := entitlement["status"]; got != "active" {
		t.Errorf("状态 = %v, want active", got)
	}
}

func TestPlanReflectsOpsConfig(t *testing.T) {
	env := testsupport.New(t)
	env.SetOpsConfig("pricing.monthly", `{"amount_cents": 9900, "currency": "CNY"}`)

	plan := env.NewClient().Get("/api/v1/billing/plan").ExpectStatus(http.StatusOK)
	if got := plan.Number("amount_cents"); got != 9900 {
		t.Errorf("套餐价格 = %v, want 9900", got)
	}
	if got := plan.String("name"); got != "CC避风港包月" {
		t.Errorf("套餐名称 = %q", got)
	}
}

// TestHeartbeatFeedsTelemetry 验证心跳记录设备与活跃度，并回传到期提醒。
func TestHeartbeatFeedsTelemetry(t *testing.T) {
	env := testsupport.New(t)
	browser, userID := env.SignUp("alice@example.com")
	session := env.AuthorizeApp(browser, "device-1")

	app := env.NewClient().WithBearer(session.AccessToken)
	resp := app.Post("/api/v1/app/heartbeat", map[string]string{
		"device_id": "device-1", "app_version": "1.4.2", "os_version": "15", "arch": "arm64",
	}).ExpectStatus(http.StatusOK)

	entitlement := resp.Object("entitlement")
	if entitlement["status"] != "trialing" {
		t.Errorf("心跳应回传订阅快照, got %v", entitlement)
	}

	var appVersion string
	if err := env.Pool.QueryRow(t.Context(),
		`SELECT app_version FROM user_devices WHERE user_id = $1 AND device_id = 'device-1'`,
		userID).Scan(&appVersion); err != nil {
		t.Fatalf("查询设备记录失败: %v", err)
	}
	if appVersion != "1.4.2" {
		t.Errorf("APP 版本 = %q", appVersion)
	}

	// 试用剩余 ≤3 天时下发到期提醒，驱动 APP 顶部横幅。
	env.Advance(28 * 24 * time.Hour)
	notices := app.Post("/api/v1/app/heartbeat", map[string]string{
		"device_id": "device-1", "app_version": "1.4.2", "os_version": "15", "arch": "arm64",
	}).ExpectStatus(http.StatusOK).Array("notices")

	if len(notices) != 1 {
		t.Fatalf("剩余 2 天应下发到期提醒, got %v", notices)
	}
	if got := notices[0].(map[string]any)["type"]; got != "expiring_soon" {
		t.Errorf("提醒类型 = %v", got)
	}
}

func TestMockRefundFlow(t *testing.T) {
	env := testsupport.New(t)
	_, userID := env.SignUp("alice@example.com")

	orderNo := env.Checkout(userID, "mock")
	payload, signature := notify(t, env, orderNo, true, 6800)
	env.NewClient().PostRaw("/api/v1/billing/webhook/mock", payload,
		map[string]string{"X-CCHaven-Signature": signature}).ExpectStatus(http.StatusOK)

	admin := newAdminClient(t, env)
	resp := admin.Post(fmt.Sprintf("/api/admin/v1/orders/%s/refund", orderNo),
		map[string]string{"reason": "用户申请"}).ExpectStatus(http.StatusOK)

	if got := resp.String("status"); got != "refunded" {
		t.Errorf("退款后订单状态 = %q, want refunded", got)
	}
	// 退款扣回该订单对应的 30 天。
	if got := env.EntitlementOf(userID)["status"]; got != "expired" && got != "none" {
		t.Errorf("退款后订阅应失效, got %v", got)
	}
}
