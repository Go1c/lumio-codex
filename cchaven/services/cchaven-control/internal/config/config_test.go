package config

import (
	"net/http"
	"slices"
	"strings"
	"testing"
	"time"
)

// setBaseEnv 铺好一份能让 Load 成功返回的最小环境。
func setBaseEnv(t *testing.T) {
	t.Helper()
	t.Setenv("CCHAVEN_DATABASE_URL", "postgres://localhost/cchaven")
	t.Setenv("CCHAVEN_JWT_SECRET", strings.Repeat("j", 32))
	t.Setenv("CCHAVEN_CODE_PEPPER", strings.Repeat("p", 32))
	t.Setenv("CCHAVEN_TOTP_KEY", strings.Repeat("k", 32))
}

func TestLoadDefaults(t *testing.T) {
	setBaseEnv(t)

	cfg, err := Load()
	if err != nil {
		t.Fatalf("Load() 失败: %v", err)
	}

	if cfg.CookieSameSite != http.SameSiteLaxMode {
		t.Errorf("默认 SameSite = %v, want Lax", cfg.CookieSameSite)
	}
	if cfg.SecureCookies {
		t.Error("dev 环境默认不应要求 Secure cookie")
	}
	// dev 下后台有默认地址，本地开发无需额外配置就能同时跑官网与后台。
	if cfg.AdminURL != devAdminURL {
		t.Errorf("dev 环境 AdminURL = %q, want %q", cfg.AdminURL, devAdminURL)
	}
	want := []string{cfg.PublicURL, devAdminURL, DefaultPortalURL}
	if got := cfg.TrustedOrigins(); !slices.Equal(got, want) {
		t.Errorf("可信来源 = %v, want %v", got, want)
	}
}

// TestLoadIdentityDefaults 锁住身份收口后的默认值：漏配也必须指向线上账号中心，
// 而不是空地址（空地址会让令牌校验对所有人失败，且错误现象极难定位）。
func TestLoadIdentityDefaults(t *testing.T) {
	setBaseEnv(t)

	cfg, err := Load()
	if err != nil {
		t.Fatalf("Load() 失败: %v", err)
	}

	if cfg.Sub2APIBase != DefaultSub2APIBase {
		t.Errorf("Sub2APIBase = %q, want %q", cfg.Sub2APIBase, DefaultSub2APIBase)
	}
	if cfg.PortalURL != DefaultPortalURL {
		t.Errorf("PortalURL = %q, want %q", cfg.PortalURL, DefaultPortalURL)
	}
	if cfg.Sub2APICacheTTL != DefaultSub2APICacheTTL {
		t.Errorf("Sub2APICacheTTL = %v, want %v", cfg.Sub2APICacheTTL, DefaultSub2APICacheTTL)
	}
	// 充值页由 Sub2API 地址推导，不在别处再硬编码一份。
	if got, want := cfg.PurchaseURL(), "https://api.lumio.games/purchase"; got != want {
		t.Errorf("PurchaseURL() = %q, want %q", got, want)
	}
	if got, want := cfg.PortalLoginURL(), "https://bestcodex.app/login"; got != want {
		t.Errorf("PortalLoginURL() = %q, want %q", got, want)
	}
}

func TestLoadIdentityOverridesTrimTrailingSlash(t *testing.T) {
	setBaseEnv(t)
	t.Setenv("CCHAVEN_SUB2API_BASE", "https://staging-api.lumio.games/")
	t.Setenv("CCHAVEN_PORTAL_URL", "https://staging.bestcodex.app/")
	t.Setenv("CCHAVEN_SUB2API_CACHE_TTL", "15s")

	cfg, err := Load()
	if err != nil {
		t.Fatalf("Load() 失败: %v", err)
	}

	if cfg.Sub2APIBase != "https://staging-api.lumio.games" {
		t.Errorf("Sub2APIBase = %q", cfg.Sub2APIBase)
	}
	if cfg.PortalURL != "https://staging.bestcodex.app" {
		t.Errorf("PortalURL = %q", cfg.PortalURL)
	}
	if cfg.Sub2APICacheTTL != 15*time.Second {
		t.Errorf("Sub2APICacheTTL = %v, want 15s", cfg.Sub2APICacheTTL)
	}
	if got, want := cfg.PurchaseURL(), "https://staging-api.lumio.games/purchase"; got != want {
		t.Errorf("PurchaseURL() = %q, want %q", got, want)
	}
}

// TestTrustedOriginsIncludesThePortal 保证统一账号中心能跨源读 CC 的权益数据。
// 漏了它，门户在生产环境的每个请求都会被 CORS 挡下，而 dev 放行 localhost 看不出来。
func TestTrustedOriginsIncludesThePortal(t *testing.T) {
	cfg := Config{
		PublicURL: "https://cc.bestcodex.app",
		AdminURL:  "https://admin.cchaven.cn",
		PortalURL: "https://bestcodex.app",
	}

	want := []string{"https://cc.bestcodex.app", "https://admin.cchaven.cn", "https://bestcodex.app"}
	if got := cfg.TrustedOrigins(); !slices.Equal(got, want) {
		t.Errorf("可信来源 = %v, want %v", got, want)
	}
}

func TestLoadAdminURLTrimsTrailingSlash(t *testing.T) {
	setBaseEnv(t)
	t.Setenv("CCHAVEN_ADMIN_URL", "https://admin.cchaven.cn/")

	cfg, err := Load()
	if err != nil {
		t.Fatalf("Load() 失败: %v", err)
	}
	// Origin 头不带末尾斜杠，配置里的斜杠必须去掉，否则永远匹配不上。
	if cfg.AdminURL != "https://admin.cchaven.cn" {
		t.Errorf("AdminURL = %q", cfg.AdminURL)
	}
}

// TestSameSiteNoneForcesSecure 验证 none 会强制打开 Secure：
// 浏览器对 SameSite=None 的 cookie 硬性要求 Secure，否则整条 Set-Cookie 被丢弃。
func TestSameSiteNoneForcesSecure(t *testing.T) {
	setBaseEnv(t)
	t.Setenv("CCHAVEN_COOKIE_SAMESITE", "None")

	cfg, err := Load()
	if err != nil {
		t.Fatalf("Load() 失败: %v", err)
	}

	if cfg.CookieSameSite != http.SameSiteNoneMode {
		t.Errorf("SameSite = %v, want None", cfg.CookieSameSite)
	}
	if !cfg.SecureCookies {
		t.Error("SameSite=None 时即便不是 prod 也必须强制 Secure")
	}
}

func TestSameSiteRejectsUnknownValue(t *testing.T) {
	setBaseEnv(t)
	t.Setenv("CCHAVEN_COOKIE_SAMESITE", "strict")

	if _, err := Load(); err == nil {
		t.Fatal("无法识别的 SameSite 取值应让服务启动失败，而不是静默回落")
	}
}

// TestEnvRejectsUnknownValue 锁住环境足枪（QA S-12）：CCHAVEN_ENV=production 这类
// 拼写错误若静默按 dev 处理，服务会用仓库里公开的开发默认密钥签发 JWT/TOTP。
func TestEnvRejectsUnknownValue(t *testing.T) {
	for _, value := range []string{"production", "prd", "staging", "PRODUCTION"} {
		t.Setenv("CCHAVEN_ENV", value)
		setBaseEnv(t)

		_, err := Load()
		if err == nil {
			t.Fatalf("CCHAVEN_ENV=%q 应让服务启动失败，而不是静默按 dev 处理", value)
		}
		if !strings.Contains(err.Error(), "CCHAVEN_ENV") {
			t.Errorf("错误信息应指明 CCHAVEN_ENV，got %v", err)
		}
	}
}

func TestEnvAcceptsDevAndProdCaseInsensitively(t *testing.T) {
	for _, value := range []string{"dev", "prod", "PROD", "prod "} {
		t.Run(value, func(t *testing.T) {
			t.Setenv("CCHAVEN_ENV", value)
			setBaseEnv(t)

			cfg, err := Load()
			if err != nil {
				t.Fatalf("CCHAVEN_ENV=%q 应能加载: %v", value, err)
			}
			want := strings.ToLower(strings.TrimSpace(value))
			if cfg.Env != want {
				t.Errorf("Env = %q, want %q", cfg.Env, want)
			}
		})
	}
}

// TestUnsetEnvWarnsAboutDevSecrets 未显式设置 CCHAVEN_ENV 时要在启动日志里
// 提醒正在用开发默认密钥——这是「忘记设 prod」唯一能被运维看见的机会。
func TestUnsetEnvWarnsAboutDevSecrets(t *testing.T) {
	setBaseEnv(t)
	t.Setenv("CCHAVEN_ENV", "")

	cfg, err := Load()
	if err != nil {
		t.Fatalf("Load() 失败: %v", err)
	}

	warnings := cfg.Warnings()
	joined := strings.Join(warnings, "\n")
	if !strings.Contains(joined, "CCHAVEN_ENV") || !strings.Contains(joined, "dev") {
		t.Errorf("未设置 CCHAVEN_ENV 应告警开发默认密钥, got %v", warnings)
	}
}

func TestWarnings(t *testing.T) {
	cases := []struct {
		name     string
		cfg      Config
		wantAny  bool
		contains string
	}{
		{
			name:     "生产环境缺少后台地址",
			cfg:      Config{Env: "prod", PublicURL: "https://cchaven.cn", CookieSameSite: http.SameSiteLaxMode},
			wantAny:  true,
			contains: "CCHAVEN_ADMIN_URL",
		},
		{
			name: "生产环境配齐两个来源不告警",
			cfg: Config{
				Env: "prod", PublicURL: "https://cchaven.cn",
				AdminURL: "https://admin.cchaven.cn", CookieSameSite: http.SameSiteLaxMode,
			},
		},
		{
			name:     "非生产环境用 None 提示 cookie 会被丢弃",
			cfg:      Config{Env: "dev", CookieSameSite: http.SameSiteNoneMode},
			wantAny:  true,
			contains: "Secure",
		},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			warnings := tc.cfg.Warnings()

			if got := len(warnings) > 0; got != tc.wantAny {
				t.Fatalf("告警条数 = %d (%v), want any=%v", len(warnings), warnings, tc.wantAny)
			}
			if tc.contains != "" && !strings.Contains(strings.Join(warnings, "\n"), tc.contains) {
				t.Errorf("告警内容应提到 %q, got %v", tc.contains, warnings)
			}
		})
	}
}

// TestTrustedOriginsSkipsEmpty 保证未配置的来源不会以空串混进可信集合。
func TestTrustedOriginsSkipsEmpty(t *testing.T) {
	cfg := Config{PublicURL: "https://cchaven.cn"}

	if got := cfg.TrustedOrigins(); !slices.Equal(got, []string{"https://cchaven.cn"}) {
		t.Errorf("可信来源 = %v, want 只有官网一项", got)
	}
}
