package api

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/config"
)

// identityConfig 是身份收口到 Sub2API 之后的一份典型生产配置。
func identityConfig() config.Config {
	cfg := prodConfig()
	cfg.PublicURL = "https://cc.bestcodex.app"
	cfg.PortalURL = "https://bestcodex.app"
	cfg.Sub2APIBase = "https://api.lumio.games"
	return cfg
}

// call 直接打整张路由表。svc 传 nil：下面这些端点都不该碰服务层，
// 一旦有人把它们接回业务逻辑，这里会立刻 panic 而不是悄悄退化。
func call(t *testing.T, method, path string) *httptest.ResponseRecorder {
	t.Helper()

	req := httptest.NewRequest(method, path, strings.NewReader("{}"))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Origin", "https://cc.bestcodex.app")

	rec := httptest.NewRecorder()
	NewServer(nil, identityConfig()).Routes().ServeHTTP(rec, req)
	return rec
}

func decodeBody(t *testing.T, rec *httptest.ResponseRecorder) map[string]any {
	t.Helper()

	var out map[string]any
	if err := json.Unmarshal(rec.Body.Bytes(), &out); err != nil {
		t.Fatalf("响应不是 JSON: %v (原文 %s)", err, rec.Body.String())
	}
	return out
}

// TestSelfServeAuthEndpointsAreGone 锁住自有终端用户认证的下线契约。
//
// 邮箱、口令、验证码从此只属于 Lumio 账号中心。这些端点不删路由而是回 410：
// 存量客户端（旧官网构建、被缓存的 SPA）打过来时要能拿到可解释的答复，
// 而不是 404 或者一个看起来像成功的空响应。
func TestSelfServeAuthEndpointsAreGone(t *testing.T) {
	endpoints := []struct {
		method string
		path   string
	}{
		{http.MethodPost, "/api/v1/auth/register"},
		{http.MethodPost, "/api/v1/auth/verify-email"},
		{http.MethodPost, "/api/v1/auth/verification-code/resend"},
		{http.MethodPost, "/api/v1/auth/login"},
		{http.MethodPost, "/api/v1/auth/password/forgot"},
		{http.MethodGet, "/api/v1/auth/password/reset/some-token"},
		{http.MethodPost, "/api/v1/auth/password/reset"},
		{http.MethodPost, "/api/v1/auth/refresh"},
		{http.MethodPost, "/api/v1/me/password"},
		{http.MethodPost, "/api/v1/me/email-change"},
		{http.MethodPost, "/api/v1/me/email-change/verify"},
		{http.MethodDelete, "/api/v1/me/email-change"},
	}

	for _, ep := range endpoints {
		t.Run(ep.method+" "+ep.path, func(t *testing.T) {
			rec := call(t, ep.method, ep.path)

			if rec.Code != http.StatusGone {
				t.Fatalf("状态码 = %d, want 410, 响应 %s", rec.Code, rec.Body.String())
			}

			errObj, ok := decodeBody(t, rec)["error"].(map[string]any)
			if !ok {
				t.Fatalf("响应缺少 error 对象: %s", rec.Body.String())
			}
			if errObj["code"] != "auth_migrated" {
				t.Errorf("错误码 = %v, want auth_migrated", errObj["code"])
			}

			details, ok := errObj["details"].(map[string]any)
			if !ok {
				t.Fatalf("响应缺少 details: %s", rec.Body.String())
			}
			if details["portal_url"] != "https://bestcodex.app/login" {
				t.Errorf("details.portal_url = %v", details["portal_url"])
			}
			if message, _ := errObj["message"].(string); !strings.Contains(message, "bestcodex.app") {
				t.Errorf("文案应指路到账号中心, got %q", message)
			}
		})
	}
}

// TestCheckoutRedirectsToTheLumioPurchasePage 锁住计费入口的去向。
//
// CC 不再自建收银台，充值与 Codex 共用 Sub2API 的 /purchase。
// 同时给出 Location 头与 JSON 体：浏览器直接跳转，XHR 客户端读 purchase_url。
func TestCheckoutRedirectsToTheLumioPurchasePage(t *testing.T) {
	rec := call(t, http.MethodPost, "/api/v1/billing/checkout")

	if rec.Code != http.StatusSeeOther {
		t.Fatalf("状态码 = %d, want 303, 响应 %s", rec.Code, rec.Body.String())
	}
	const want = "https://api.lumio.games/purchase"
	if got := rec.Header().Get("Location"); got != want {
		t.Errorf("Location = %q, want %q", got, want)
	}

	data, ok := decodeBody(t, rec)["data"].(map[string]any)
	if !ok {
		t.Fatalf("响应缺少 data: %s", rec.Body.String())
	}
	if data["purchase_url"] != want {
		t.Errorf("data.purchase_url = %v, want %q", data["purchase_url"], want)
	}
	if data["reason"] != "billing_moved_to_lumio" {
		t.Errorf("data.reason = %v, want billing_moved_to_lumio", data["reason"])
	}
}

// TestLogoutStaysAvailable 退出登录不属于身份体系，本地会话该清还得清。
func TestLogoutStaysAvailable(t *testing.T) {
	rec := call(t, http.MethodPost, "/api/v1/auth/logout")

	if rec.Code != http.StatusNoContent {
		t.Fatalf("状态码 = %d, want 204, 响应 %s", rec.Code, rec.Body.String())
	}
}

// TestHealthStaysAvailable 兜底断言：改造没有把探针一起搞挂。
func TestHealthStaysAvailable(t *testing.T) {
	if rec := call(t, http.MethodGet, "/api/v1/health"); rec.Code != http.StatusOK {
		t.Fatalf("健康检查状态码 = %d, want 200", rec.Code)
	}
}
