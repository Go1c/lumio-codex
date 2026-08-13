package store

import "testing"

// TestAllowsRedirectURI 锁定回调地址白名单逻辑。
//
// 桌面端回环回调的端口是随机的，所以模式里用 * 通配端口；除此之外必须精确匹配，
// 否则会退化成开放重定向，授权码可被第三方站点截获。
func TestAllowsRedirectURI(t *testing.T) {
	client := OAuthClient{
		RedirectURIPatterns: []string{
			"http://127.0.0.1:*/callback",
			"http://localhost:*/callback",
			"cchaven://auth/callback",
		},
	}

	allowed := []string{
		"http://127.0.0.1:53682/callback",
		"http://127.0.0.1:1/callback",
		"http://localhost:8080/callback",
		"cchaven://auth/callback",
	}
	for _, uri := range allowed {
		if !client.AllowsRedirectURI(uri) {
			t.Errorf("应放行 %q", uri)
		}
	}

	rejected := []string{
		"http://evil.com/callback",
		"https://127.0.0.1:53682/callback",         // 协议不符
		"http://127.0.0.1:53682/callback/../steal", // 路径不符
		"http://127.0.0.1:53682/other",
		"http://127.0.0.1.evil.com:80/callback",
		"cchaven://auth/other",
		"",
	}
	for _, uri := range rejected {
		if client.AllowsRedirectURI(uri) {
			t.Errorf("应拒绝 %q", uri)
		}
	}
}

func TestAllowsScope(t *testing.T) {
	client := OAuthClient{Scopes: []string{"profile", "workspace", "offline_access"}}

	if !client.AllowsScope("profile workspace") {
		t.Error("子集 scope 应放行")
	}
	if !client.AllowsScope("") {
		t.Error("空 scope 在此函数层面视为放行，由上层单独校验非空")
	}
	if client.AllowsScope("profile admin") {
		t.Error("超出注册范围的 scope 应拒绝")
	}
}
