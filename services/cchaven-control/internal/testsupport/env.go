package testsupport

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/api"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/config"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/db"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/payments"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/security"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/service"
	"github.com/Go1c/fns-workspace/services/cchaven-control/migrations"
)

// mockSecret 是测试中 mock 支付渠道的验签密钥，同时充当 JWT 密钥。
var mockSecret = []byte("test-secret-key-at-least-32-bytes!!")

// Env 是一次测试的完整运行环境。
type Env struct {
	T      *testing.T
	Pool   *db.Pool
	Svc    *service.Service
	Cfg    config.Config
	Mock   *payments.Mock
	Server *httptest.Server

	clock time.Time
}

// New 建立测试环境：连接测试库、清空数据、装配服务与 HTTP 服务器。
//
// 每个测试都从干净的数据库开始，但共用同一个 PostgreSQL 实例，
// 因此不能并行运行（未调用 t.Parallel）。
func New(t *testing.T) *Env {
	t.Helper()

	url, err := StartPostgres()
	if err != nil {
		t.Fatalf("启动测试数据库失败: %v", err)
	}

	ctx := context.Background()
	pool, err := db.Connect(ctx, url)
	if err != nil {
		t.Fatalf("连接测试数据库失败: %v", err)
	}
	t.Cleanup(pool.Close)

	if err := MigrateOnce(ctx, pool); err != nil {
		t.Fatalf("执行迁移失败: %v", err)
	}
	resetDatabase(t, pool)

	cfg := config.Config{
		Env: "test",
		// 官网与管理后台是两个独立来源，测试环境照生产的样子摆开，
		// 免得「只有官网能过同源校验」这类问题又躲过集成测试。
		PublicURL: "https://cchaven.test",
		AdminURL:  "https://admin.cchaven.test",
		CookieName: config.CookieNames{
			Session: "cch_sess", Refresh: "cch_refresh",
			Referral: "cch_ref", Admin: "cch_admin",
		},
		CookieSameSite:  http.SameSiteLaxMode,
		JWTSecret:       mockSecret,
		CodePepper:      mockSecret,
		TOTPSecretKey:   mockSecret,
		AccessTokenTTL:  15 * time.Minute,
		RefreshTokenTTL: 30 * 24 * time.Hour,
		WebSessionTTL:   30 * 24 * time.Hour,
		AdminSessionTTL: 12 * time.Hour,
	}

	cipher, err := security.NewCipher(cfg.TOTPSecretKey)
	if err != nil {
		t.Fatalf("构造加密器失败: %v", err)
	}

	mock := payments.NewMock(cfg.PublicURL, cfg.JWTSecret)
	registry := payments.NewRegistry()
	registry.Register(mock)

	// 测试用低代价 Argon2 参数，否则大量注册会把测试拖到分钟级。
	svc := service.New(pool, cfg, security.NewHasher(security.TestArgon2Params()), cipher, registry)

	env := &Env{T: t, Pool: pool, Svc: svc, Cfg: cfg, Mock: mock, clock: time.Now().UTC()}
	svc.Now = func() time.Time { return env.clock }

	env.Server = httptest.NewServer(api.NewServer(svc, cfg).Routes())
	t.Cleanup(env.Server.Close)

	return env
}

// Now 返回测试时钟的当前时刻。
func (e *Env) Now() time.Time { return e.clock }

// Advance 让测试时钟前进，用于验证过期、冷却与到期计算。
func (e *Env) Advance(d time.Duration) { e.clock = e.clock.Add(d) }

// SetClock 把测试时钟设为指定时刻。
func (e *Env) SetClock(t time.Time) { e.clock = t.UTC() }

// truncatedTables 是每个测试开始前需要清空的业务表。
// schema_migrations 不在其列——表结构本身跨测试复用。
var truncatedTables = []string{
	"audit_logs", "email_outbox", "payment_events", "refunds", "orders", "order_sequences",
	"user_activity_days", "user_devices", "trial_fingerprints", "referral_attributions",
	"referral_visits", "referral_codes", "subscription_events", "subscriptions",
	"oauth_authorization_codes", "refresh_tokens", "session_families",
	"password_reset_tokens", "email_verification_codes", "admin_sessions", "admins",
	"app_releases", "ops_configs", "oauth_clients", "users",
}

func resetDatabase(t *testing.T, pool *db.Pool) {
	t.Helper()
	ctx := context.Background()

	statement := "TRUNCATE TABLE "
	for i, table := range truncatedTables {
		if i > 0 {
			statement += ", "
		}
		statement += table
	}
	statement += " RESTART IDENTITY CASCADE"

	if _, err := pool.Exec(ctx, statement); err != nil {
		t.Fatalf("清空测试数据失败: %v", err)
	}

	// 种子数据（运营配置默认值、桌面端 OAuth 客户端）随 TRUNCATE 一起被清掉，需重新灌入。
	seed, err := migrations.FS.ReadFile("0002_seed.sql")
	if err != nil {
		t.Fatalf("读取种子脚本失败: %v", err)
	}
	if _, err := pool.Exec(ctx, string(seed)); err != nil {
		t.Fatalf("灌入种子数据失败: %v", err)
	}
}
