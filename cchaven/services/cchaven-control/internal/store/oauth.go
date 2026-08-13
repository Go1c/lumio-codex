package store

import (
	"context"
	"path"
	"strings"
	"time"
)

// OAuthClient 是已注册的 OAuth 客户端。
type OAuthClient struct {
	ID                  string
	Name                string
	RedirectURIPatterns []string
	IsPublic            bool
	Scopes              []string
}

// GetOAuthClient 读取客户端注册信息。
func GetOAuthClient(ctx context.Context, q Querier, id string) (OAuthClient, error) {
	var c OAuthClient
	err := q.QueryRow(ctx, `
		SELECT id, name, redirect_uri_patterns, is_public, scopes
		  FROM oauth_clients WHERE id = $1`, id).
		Scan(&c.ID, &c.Name, &c.RedirectURIPatterns, &c.IsPublic, &c.Scopes)
	if err != nil {
		return OAuthClient{}, normalizeErr(err)
	}
	return c, nil
}

// AllowsRedirectURI 报告回调地址是否匹配任一注册模式。
//
// 桌面端有两种回调：本机回环（端口随机，故模式里用 * 通配端口）与自定义 scheme 兜底。
// 除通配端口外一律精确匹配，避免开放重定向。
func (c OAuthClient) AllowsRedirectURI(uri string) bool {
	for _, pattern := range c.RedirectURIPatterns {
		if matchRedirectPattern(pattern, uri) {
			return true
		}
	}
	return false
}

func matchRedirectPattern(pattern, uri string) bool {
	if !strings.Contains(pattern, "*") {
		return pattern == uri
	}
	// path.Match 的 * 不跨 '/'，正好限制通配只发生在端口段。
	ok, err := path.Match(pattern, uri)
	return err == nil && ok
}

// AllowsScope 报告请求的 scope 是否都在客户端允许范围内。
func (c OAuthClient) AllowsScope(scope string) bool {
	allowed := make(map[string]bool, len(c.Scopes))
	for _, s := range c.Scopes {
		allowed[s] = true
	}
	for _, s := range strings.Fields(scope) {
		if !allowed[s] {
			return false
		}
	}
	return true
}

// AuthorizationCode 是一枚已签发的授权码。
type AuthorizationCode struct {
	ClientID      string
	UserID        int64
	RedirectURI   string
	Scope         string
	CodeChallenge string
	DeviceName    string
	Platform      string
	OSVersion     string
	Arch          string
	AppVersion    string
	ExpiresAt     time.Time
	ConsumedAt    *time.Time
}

// CreateAuthorizationCode 落库授权码摘要。明文只回给浏览器，不入库。
func CreateAuthorizationCode(ctx context.Context, q Querier, codeHash string, c AuthorizationCode) error {
	_, err := q.Exec(ctx, `
		INSERT INTO oauth_authorization_codes
		    (code_hash, client_id, user_id, redirect_uri, scope, code_challenge,
		     code_challenge_method, device_name, platform, os_version, arch, app_version, expires_at)
		VALUES ($1, $2, $3, $4, $5, $6, 'S256', $7, $8, $9, $10, $11, $12)`,
		codeHash, c.ClientID, c.UserID, c.RedirectURI, c.Scope, c.CodeChallenge,
		c.DeviceName, c.Platform, c.OSVersion, c.Arch, c.AppVersion, c.ExpiresAt)
	return err
}

// ConsumeAuthorizationCode 原子地取出并标记授权码为已用。
//
// 用一条带 WHERE consumed_at IS NULL 的 UPDATE ... RETURNING 完成，
// 使得并发兑换只有一方成功，杜绝授权码重放。
func ConsumeAuthorizationCode(
	ctx context.Context, q Querier, codeHash string, now time.Time,
) (AuthorizationCode, error) {
	var c AuthorizationCode
	err := q.QueryRow(ctx, `
		UPDATE oauth_authorization_codes
		   SET consumed_at = $2
		 WHERE code_hash = $1 AND consumed_at IS NULL AND expires_at > $2
		RETURNING client_id, user_id, redirect_uri, scope, code_challenge,
		          device_name, platform, os_version, arch, app_version, expires_at`,
		codeHash, now).Scan(
		&c.ClientID, &c.UserID, &c.RedirectURI, &c.Scope, &c.CodeChallenge,
		&c.DeviceName, &c.Platform, &c.OSVersion, &c.Arch, &c.AppVersion, &c.ExpiresAt)
	if err != nil {
		return AuthorizationCode{}, normalizeErr(err)
	}
	return c, nil
}

// DeleteExpiredAuthorizationCodes 清理过期授权码，由后台任务调用。
func DeleteExpiredAuthorizationCodes(ctx context.Context, q Querier, before time.Time) (int64, error) {
	tag, err := q.Exec(ctx, `DELETE FROM oauth_authorization_codes WHERE expires_at < $1`, before)
	if err != nil {
		return 0, err
	}
	return tag.RowsAffected(), nil
}

// HasAppSession 报告用户是否已建立过桌面 APP 会话。
// 「首次登录 APP」是邀请闭环的触发点，需要据此判定。
func HasAppSession(ctx context.Context, q Querier, userID int64) (bool, error) {
	var exists bool
	err := q.QueryRow(ctx,
		`SELECT EXISTS (SELECT 1 FROM session_families WHERE user_id = $1 AND client = 'app')`,
		userID).Scan(&exists)
	return exists, err
}
