package config

import (
	"net/http"
	"slices"
	"strings"
	"testing"
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
	if got := cfg.TrustedOrigins(); !slices.Equal(got, []string{cfg.PublicURL, devAdminURL}) {
		t.Errorf("可信来源 = %v, want 官网与后台两项", got)
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
