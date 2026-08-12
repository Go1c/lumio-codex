package testsupport

import (
	"context"
	"crypto/sha256"
	"encoding/base64"
	"net/http"
	"net/url"
	"strings"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/security"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/store"
)

// DesktopRedirectURI 是桌面端注册的回环回调地址。
const DesktopRedirectURI = "http://127.0.0.1:53682/callback"

// PKCE 是一组 PKCE 参数。
type PKCE struct {
	Verifier  string
	Challenge string
}

// NewPKCE 生成一组符合 RFC 7636 的 S256 参数。
func NewPKCE(seed string) PKCE {
	// verifier 需要 43–128 个字符，用种子补足长度以便不同测试拿到不同的值。
	verifier := (seed + strings.Repeat("0123456789abcdefghijklmnopqrstuvwxyz", 4))[:64]
	sum := sha256.Sum256([]byte(verifier))
	return PKCE{Verifier: verifier, Challenge: base64.RawURLEncoding.EncodeToString(sum[:])}
}

// Register 提交注册并返回验证码（非生产环境由接口回传）。
func (e *Env) Register(c *Client, email, password string) string {
	e.T.Helper()
	resp := c.Post("/api/v1/auth/register", map[string]string{
		"email": email, "password": password,
	}).ExpectStatus(http.StatusCreated)
	return resp.String("dev_code")
}

// VerifyEmail 提交验证码，成功后客户端持有官网会话 cookie。
func (e *Env) VerifyEmail(c *Client, email, code string) {
	e.T.Helper()
	c.Post("/api/v1/auth/verify-email", map[string]string{
		"email": email, "code": code,
	}).ExpectStatus(http.StatusOK)
}

// SignUp 完成「注册 + 邮箱验证」，返回已登录的浏览器会话与用户 ID。
func (e *Env) SignUp(email, password string) (*Client, int64) {
	e.T.Helper()

	client := e.NewClient()
	code := e.Register(client, email, password)
	e.VerifyEmail(client, email, code)
	return client, e.UserIDOf(email)
}

// UserIDOf 按邮箱查出用户 ID。
func (e *Env) UserIDOf(email string) int64 {
	e.T.Helper()

	var id int64
	if err := e.Pool.QueryRow(context.Background(),
		`SELECT id FROM users WHERE lower(email) = lower($1)`, email).Scan(&id); err != nil {
		e.T.Fatalf("查询用户 %s 失败: %v", email, err)
	}
	return id
}

// ReferralCodeOf 取出用户的邀请码。
func (e *Env) ReferralCodeOf(userID int64) string {
	e.T.Helper()

	var code string
	if err := e.Pool.QueryRow(context.Background(),
		`SELECT code FROM referral_codes WHERE user_id = $1`, userID).Scan(&code); err != nil {
		e.T.Fatalf("查询用户 %d 的邀请码失败: %v", userID, err)
	}
	return code
}

// AppSession 是一次桌面端授权的结果。
type AppSession struct {
	AccessToken  string
	RefreshToken string
	Activation   map[string]any
	Entitlement  map[string]any
}

// AuthorizeApp 走完整的「浏览器登录 → 授权 → 换取令牌」链路。
//
// c 必须是已登录官网的浏览器会话，模拟 APP 打开系统浏览器后的授权确认。
func (e *Env) AuthorizeApp(c *Client, deviceID string) AppSession {
	e.T.Helper()

	pkce := NewPKCE(deviceID)
	query := url.Values{
		"client_id":             {"cchaven-desktop"},
		"redirect_uri":          {DesktopRedirectURI},
		"scope":                 {"profile workspace offline_access"},
		"code_challenge":        {pkce.Challenge},
		"code_challenge_method": {"S256"},
		"state":                 {"state-" + deviceID},
	}

	approve := c.Post("/api/v1/oauth/authorize?"+query.Encode(), map[string]string{
		"device_name": "MacBook Pro",
		"os_version":  "15",
		"arch":        "arm64",
		"app_version": "1.4.2",
	}).ExpectStatus(http.StatusOK)

	// 授权码兑换不依赖浏览器 cookie，用独立客户端模拟桌面 APP 发起。
	app := e.NewClient()
	token := app.Post("/api/v1/oauth/token", map[string]string{
		"grant_type":    "authorization_code",
		"code":          approve.String("code"),
		"code_verifier": pkce.Verifier,
		"client_id":     "cchaven-desktop",
		"redirect_uri":  DesktopRedirectURI,
		"device_id":     deviceID,
	}).ExpectStatus(http.StatusOK)

	session := AppSession{
		AccessToken:  token.String("access_token"),
		RefreshToken: token.String("refresh_token"),
	}
	if activation, ok := token.Data()["activation"].(map[string]any); ok {
		session.Activation = activation
	}
	if entitlement, ok := token.Data()["entitlement"].(map[string]any); ok {
		session.Entitlement = entitlement
	}
	return session
}

// EntitlementOf 读取指定用户的订阅快照。
func (e *Env) EntitlementOf(userID int64) map[string]any {
	e.T.Helper()

	snapshot, err := e.Svc.Entitlement(context.Background(), userID)
	if err != nil {
		e.T.Fatalf("读取订阅快照失败: %v", err)
	}
	return map[string]any{
		"status":           string(snapshot.Status),
		"kind":             snapshot.Kind,
		"days_left":        snapshot.DaysLeft,
		"bonus_days_total": snapshot.BonusDaysTotal,
	}
}

// OutboxTemplates 列出发件箱中投递给指定邮箱的模板名，用于断言通知是否发出。
func (e *Env) OutboxTemplates(email string) []string {
	e.T.Helper()

	rows, err := e.Pool.Query(context.Background(),
		`SELECT template FROM email_outbox WHERE to_email = $1 ORDER BY id`, email)
	if err != nil {
		e.T.Fatalf("读取发件箱失败: %v", err)
	}
	defer rows.Close()

	var out []string
	for rows.Next() {
		var template string
		if err := rows.Scan(&template); err != nil {
			e.T.Fatalf("读取发件箱失败: %v", err)
		}
		out = append(out, template)
	}
	return out
}

// CreateAdmin 建立一个 owner 角色的管理后台账号（尚未启用两步验证）。
func (e *Env) CreateAdmin(email, password string) int64 {
	e.T.Helper()
	return e.CreateAdminWithRole(email, password, "owner")
}

// CreateAdminWithRole 建立指定角色的管理后台账号，用于验证按角色分级的权限。
func (e *Env) CreateAdminWithRole(email, password, role string) int64 {
	e.T.Helper()

	hash, err := security.NewHasher(security.TestArgon2Params()).Hash(password)
	if err != nil {
		e.T.Fatalf("生成管理员口令哈希失败: %v", err)
	}

	admin, err := store.CreateAdmin(context.Background(), e.Pool, email, hash, "运营", role)
	if err != nil {
		e.T.Fatalf("创建管理员失败: %v", err)
	}
	return admin.ID
}

// SetOpsConfig 直接改写运营配置，便于测试「奖励天数配 0 即关闭」等分支。
func (e *Env) SetOpsConfig(key string, jsonValue string) {
	e.T.Helper()

	if _, err := e.Pool.Exec(context.Background(), `
		INSERT INTO ops_configs (key, value) VALUES ($1, $2::jsonb)
		ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value`, key, jsonValue); err != nil {
		e.T.Fatalf("写入运营配置失败: %v", err)
	}
}
