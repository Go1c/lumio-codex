// Package config 从环境变量装载服务配置（12-factor）。
package config

import (
	"fmt"
	"net/http"
	"os"
	"strconv"
	"strings"
	"time"
)

// devAdminURL 是管理后台的本地开发地址，取自 apps/admin/vite.config.ts 的 server.port。
const devAdminURL = "http://localhost:5183"

// 身份收口到 Sub2API 之后的默认地址。
const (
	// DefaultSub2APIBase 是 Lumio 账号中心（Sub2API）的线上地址，
	// 终端用户的邮箱、口令与账号状态全部由它保管。
	DefaultSub2APIBase = "https://api.lumio.games"
	// DefaultPortalURL 是统一门户，注册 / 登录 / 找回密码都在这里完成。
	DefaultPortalURL = "https://lumiogame.com"
	// DefaultSub2APICacheTTL 是身份校验结果的缓存时长。
	DefaultSub2APICacheTTL = time.Minute
	// purchasePath 是 Sub2API 的充值页路径；CC 与 Codex 共用同一个收银入口。
	purchasePath = "/purchase"
)

// Config 是服务的完整运行配置。
type Config struct {
	Env        string // dev | prod
	HTTPAddr   string
	PublicURL  string // CC 产品站地址，用于拼接邀请链接与支付回跳
	AdminURL   string // 管理后台地址；后台独立部署，不是官网的子路径
	PortalURL  string // 统一门户（账号中心）地址，自有认证接口的 410 响应指向它
	CookieName CookieNames

	// envExplicit 记录 CCHAVEN_ENV 是否被显式设置；未设置时 Warnings 要提醒
	// 当前跑在开发默认密钥上（QA S-12 的足枪）。
	envExplicit bool

	// Sub2APIBase 是身份真源的地址；Sub2APICacheTTL 是校验结果的缓存时长。
	Sub2APIBase     string
	Sub2APICacheTTL time.Duration

	DatabaseURL string

	// Secrets 全部来自环境变量，绝不落配置文件。
	JWTSecret     []byte
	CodePepper    []byte // 验证码摘要的 pepper
	TOTPSecretKey []byte // 加密管理员 TOTP 种子的 AES-256 密钥

	AccessTokenTTL  time.Duration
	RefreshTokenTTL time.Duration
	// SessionAbsoluteTTL 是会话族的绝对寿命上限（与滑动续期的 RefreshTokenTTL 相对）。
	//
	// refresh 轮换让「持续使用的会话永不失效」，而上游（Sub2API）的停用决策没有
	// 主动同步通道；绝对上限把伤害收敛到「至多 TTL 后失效」，到期须重新走浏览器
	// 授权——那条链路每次都会回源校验账号状态（QA S-2）。
	SessionAbsoluteTTL time.Duration
	WebSessionTTL      time.Duration
	AdminSessionTTL    time.Duration

	SecureCookies  bool
	CookieSameSite http.SameSite

	SMTP SMTPConfig
}

// TrustedOrigins 返回允许跨源访问控制面的前端来源。
//
// 三个，因为本系统面向三套独立部署、互不相干的前端：
//   - PublicURL —— CC 产品站（cc.lumiogame.com）
//   - AdminURL  —— 管理后台 apps/admin（admin.cchaven.cn）。交互设计第 7 章要求后台
//     与官网、APP 完全隔离，它不是官网的子路径，因此必须单独列为可信来源，
//     否则后台在生产环境会被 CORS 与写操作的同源校验一起挡死。
//   - PortalURL —— 统一门户 / 账号中心（lumiogame.com）。它拿 Sub2API 令牌来读 CC 的
//     权益与邀请数据，不在这里放行的话生产环境会被 CORS 全量挡下。
//
// CORS 响应头与 cookie 写操作的 CSRF 校验都读这一处，两者永远一致。
// 将来再加前端（比如独立的状态页），在 Config 里加字段并追加到这里即可，
// 不要在 api 层另开一个 if 分支。
func (c Config) TrustedOrigins() []string {
	origins := make([]string, 0, 3)
	for _, origin := range []string{c.PublicURL, c.AdminURL, c.PortalURL} {
		if origin != "" {
			origins = append(origins, origin)
		}
	}
	return origins
}

// PurchaseURL 返回统一充值页。CC 与 Codex 都跳这里，本服务不再自建收银台。
func (c Config) PurchaseURL() string {
	return baseOr(c.Sub2APIBase, DefaultSub2APIBase) + purchasePath
}

// PortalLoginURL 返回账号中心的登录页，供已下线的自有认证接口指路。
func (c Config) PortalLoginURL() string {
	return baseOr(c.PortalURL, DefaultPortalURL) + "/login"
}

func baseOr(value, fallback string) string {
	if trimmed := strings.TrimRight(strings.TrimSpace(value), "/"); trimmed != "" {
		return trimmed
	}
	return fallback
}

// Warnings 返回启动时应当提醒运维、但还不足以阻止服务启动的配置问题。
// 由 main 在日志初始化之后逐条打出，避免这些坑要等到线上报障才被发现。
func (c Config) Warnings() []string {
	var out []string

	// 漏设环境变量的部署会静默用仓库里公开的开发默认密钥签 JWT/TOTP、cookie
	// 不带 Secure——这等于任何人都能伪造令牌，必须在启动时喊出来（QA S-12）。
	if c.Env != "prod" && !c.envExplicit {
		out = append(out, "CCHAVEN_ENV 未设置，当前按 dev 运行："+
			"JWT/TOTP/pepper 回落到公开的开发默认密钥，cookie 不带 Secure；"+
			"生产部署必须显式设置 CCHAVEN_ENV=prod 并配置全部密钥")
	}

	if c.Env == "prod" && c.AdminURL == "" {
		out = append(out, "未配置 CCHAVEN_ADMIN_URL：管理后台的来源不在可信集合里，"+
			"浏览器会拦下它的跨源请求，后台的所有写操作都会返回 403")
	}
	// Secure cookie 在明文 HTTP 上会被浏览器丢弃（http://localhost 是各浏览器的特例，
	// 生产域名没有这个豁免）。SameSite=None 强制开了 Secure，非 prod 环境下要提醒一句。
	if c.CookieSameSite == http.SameSiteNoneMode && c.Env != "prod" {
		out = append(out, "CCHAVEN_COOKIE_SAMESITE=none 已强制 Secure=true："+
			"当前环境不是 prod，若控制面没走 HTTPS，浏览器会直接丢弃会话 cookie，登录将无法保持")
	}

	return out
}

// CookieNames 集中管理 cookie 名称，避免各处硬编码。
type CookieNames struct {
	Session  string
	Refresh  string
	Referral string
	Admin    string
}

// SMTPConfig 为空时邮件只入 email_outbox 不实际投递（开发与测试默认行为）。
type SMTPConfig struct {
	Host     string
	Port     int
	Username string
	Password string
	From     string
}

// Enabled 报告是否配置了可用的 SMTP 服务器。
func (s SMTPConfig) Enabled() bool { return s.Host != "" }

// Load 读取环境变量并校验必填项。
func Load() (Config, error) {
	environment, envExplicit, err := parseEnv(os.Getenv("CCHAVEN_ENV"))
	if err != nil {
		return Config{}, err
	}

	sameSite, err := parseSameSite(env("CCHAVEN_COOKIE_SAMESITE", "lax"))
	if err != nil {
		return Config{}, err
	}

	// 后台地址只在 dev 有默认值：本地开发一定跑在 vite 端口上，猜得准；
	// 生产的后台域名无从推断，宁可留空并在启动时告警，也不要猜一个错的放进可信来源。
	adminURL := strings.TrimRight(env("CCHAVEN_ADMIN_URL", ""), "/")
	if adminURL == "" && environment == "dev" {
		adminURL = devAdminURL
	}

	cfg := Config{
		Env:         environment,
		envExplicit: envExplicit,
		HTTPAddr:    env("CCHAVEN_HTTP_ADDR", ":8080"),
		PublicURL:   strings.TrimRight(env("CCHAVEN_PUBLIC_URL", "http://localhost:5173"), "/"),
		AdminURL:    adminURL,
		PortalURL:   strings.TrimRight(env("CCHAVEN_PORTAL_URL", DefaultPortalURL), "/"),
		Sub2APIBase: strings.TrimRight(
			env("CCHAVEN_SUB2API_BASE", DefaultSub2APIBase), "/"),
		Sub2APICacheTTL: duration("CCHAVEN_SUB2API_CACHE_TTL", DefaultSub2APICacheTTL),
		CookieName: CookieNames{
			Session:  "cch_sess",
			Refresh:  "cch_refresh",
			Referral: "cch_ref",
			Admin:    "cch_admin",
		},
		DatabaseURL:        env("CCHAVEN_DATABASE_URL", ""),
		AccessTokenTTL:     duration("CCHAVEN_ACCESS_TOKEN_TTL", 15*time.Minute),
		RefreshTokenTTL:    duration("CCHAVEN_REFRESH_TOKEN_TTL", 60*24*time.Hour),
		SessionAbsoluteTTL: duration("CCHAVEN_SESSION_ABSOLUTE_TTL", 14*24*time.Hour),
		WebSessionTTL:      duration("CCHAVEN_WEB_SESSION_TTL", 30*24*time.Hour),
		AdminSessionTTL:    duration("CCHAVEN_ADMIN_SESSION_TTL", 12*time.Hour),
		SMTP: SMTPConfig{
			Host:     env("CCHAVEN_SMTP_HOST", ""),
			Port:     integer("CCHAVEN_SMTP_PORT", 587),
			Username: env("CCHAVEN_SMTP_USERNAME", ""),
			Password: env("CCHAVEN_SMTP_PASSWORD", ""),
			From:     env("CCHAVEN_SMTP_FROM", "CC避风港 <no-reply@cchaven.cn>"),
		},
	}
	cfg.CookieSameSite = sameSite
	// SameSite=None 的 cookie 浏览器一律要求同时带 Secure，否则整条 Set-Cookie 被忽略。
	// 与其让运维在「登录莫名其妙不生效」上排查半天，不如在这里直接兜住。
	cfg.SecureCookies = cfg.Env == "prod" || sameSite == http.SameSiteNoneMode

	if cfg.DatabaseURL == "" {
		return Config{}, fmt.Errorf("config: 缺少 CCHAVEN_DATABASE_URL")
	}

	if cfg.JWTSecret, err = secret("CCHAVEN_JWT_SECRET", 32, cfg.Env); err != nil {
		return Config{}, err
	}
	if cfg.CodePepper, err = secret("CCHAVEN_CODE_PEPPER", 32, cfg.Env); err != nil {
		return Config{}, err
	}
	if cfg.TOTPSecretKey, err = secret("CCHAVEN_TOTP_KEY", 32, cfg.Env); err != nil {
		return Config{}, err
	}
	return cfg, nil
}

// parseEnv 解析 CCHAVEN_ENV，返回归一化后的值与「是否被显式设置」。
//
// 拼错就启动失败（如 production / prd）：静默按 dev 处理会让服务用仓库里公开的
// 开发默认密钥签发 JWT 与 TOTP，等于向所有访问者开放伪造令牌（QA S-12）。
// 大小写不敏感只求宽容，合法值仍然只有 dev 与 prod。
func parseEnv(value string) (env string, explicit bool, err error) {
	trimmed := strings.ToLower(strings.TrimSpace(value))
	switch trimmed {
	case "dev", "prod":
		return trimmed, value != "", nil
	case "":
		return "dev", false, nil
	default:
		return "", false, fmt.Errorf("config: CCHAVEN_ENV 只接受 dev 或 prod，收到 %q；"+
			"注意 production/prd 等别名不会被识别为 prod", value)
	}
}

// parseSameSite 解析会话 cookie 的 SameSite 策略。
//
// 只提供 lax 与 none 两种：
//   - lax（默认）适用于控制面与前端同站的部署（cchaven.cn 与 api.cchaven.cn 的 eTLD+1 相同），
//     顶层导航仍会带上 cookie，同时天然挡掉跨站表单提交，最安全；
//   - none 适用于控制面被放到另一个站点（不同 eTLD+1）的部署，此时 lax 会让浏览器
//     根本不发送 cookie，登录直接失效。
//
// 不提供 strict：它连从邮件里点重设密码链接跳回来都不带 cookie，会破坏现有链路。
func parseSameSite(value string) (http.SameSite, error) {
	switch strings.ToLower(strings.TrimSpace(value)) {
	case "lax":
		return http.SameSiteLaxMode, nil
	case "none":
		return http.SameSiteNoneMode, nil
	default:
		// 拼错就启动失败：静默回落到 lax 会让「跨站部署下登录不了」变成线上谜题。
		return 0, fmt.Errorf("config: CCHAVEN_COOKIE_SAMESITE 只接受 lax 或 none，收到 %q", value)
	}
}

// secret 读取密钥；生产环境强制配置且不得短于 minLen，开发环境允许回落到固定值以便本地启动。
func secret(key string, minLen int, environment string) ([]byte, error) {
	v := os.Getenv(key)
	if v == "" {
		if environment == "prod" {
			return nil, fmt.Errorf("config: 生产环境必须配置 %s", key)
		}
		return []byte(strings.Repeat("dev-insecure-", 4)[:minLen]), nil
	}
	if len(v) < minLen {
		return nil, fmt.Errorf("config: %s 至少需要 %d 字节", key, minLen)
	}
	return []byte(v), nil
}

func env(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}

func integer(key string, fallback int) int {
	if v := os.Getenv(key); v != "" {
		if n, err := strconv.Atoi(v); err == nil {
			return n
		}
	}
	return fallback
}

func duration(key string, fallback time.Duration) time.Duration {
	if v := os.Getenv(key); v != "" {
		if d, err := time.ParseDuration(v); err == nil {
			return d
		}
	}
	return fallback
}
