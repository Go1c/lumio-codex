package store

import (
	"context"
	"time"

	"github.com/google/uuid"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/domain"
)

// CreateSessionFamilyParams 描述一次新登录。
type CreateSessionFamilyParams struct {
	UserID      int64
	Client      domain.SessionClient
	OAuthClient string
	DeviceName  string
	Platform    string
	OSVersion   string
	Arch        string
	AppVersion  string
	UserAgent   string
	IP          string
	IPRegion    string
}

// CreateSessionFamily 建立会话族。
func CreateSessionFamily(ctx context.Context, q Querier, p CreateSessionFamilyParams) (uuid.UUID, error) {
	id := uuid.New()
	_, err := q.Exec(ctx, `
		INSERT INTO session_families
		    (id, user_id, client, oauth_client_id, device_name, platform,
		     os_version, arch, app_version, user_agent, ip, ip_region)
		VALUES ($1, $2, $3, nullif($4, ''), $5, $6, $7, $8, $9, $10, $11, $12)`,
		id, p.UserID, p.Client, p.OAuthClient, p.DeviceName, p.Platform,
		p.OSVersion, p.Arch, p.AppVersion, p.UserAgent, p.IP, p.IPRegion)
	if err != nil {
		return uuid.Nil, err
	}
	return id, nil
}

// GetActiveSessionFamily 读取未撤销的会话族；已撤销或不存在都返回 ErrNotFound。
func GetActiveSessionFamily(ctx context.Context, q Querier, id uuid.UUID) (domain.SessionFamily, error) {
	var s domain.SessionFamily
	err := q.QueryRow(ctx, `
		SELECT id, user_id, client, coalesce(oauth_client_id, ''), device_name, platform,
		       os_version, arch, app_version, user_agent, ip, ip_region,
		       created_at, last_seen_at, revoked_at
		  FROM session_families
		 WHERE id = $1 AND revoked_at IS NULL`, id).Scan(
		&s.ID, &s.UserID, &s.Client, &s.OAuthClient, &s.DeviceName, &s.Platform,
		&s.OSVersion, &s.Arch, &s.AppVersion, &s.UserAgent, &s.IP, &s.IPRegion,
		&s.CreatedAt, &s.LastSeenAt, &s.RevokedAt)
	if err != nil {
		return domain.SessionFamily{}, normalizeErr(err)
	}
	return s, nil
}

// ListSessionFamilies 列出用户的全部活跃会话，供「登录设备与授权」页展示。
func ListSessionFamilies(ctx context.Context, q Querier, userID int64) ([]domain.SessionFamily, error) {
	rows, err := q.Query(ctx, `
		SELECT id, user_id, client, coalesce(oauth_client_id, ''), device_name, platform,
		       os_version, arch, app_version, user_agent, ip, ip_region,
		       created_at, last_seen_at
		  FROM session_families
		 WHERE user_id = $1 AND revoked_at IS NULL
		 ORDER BY last_seen_at DESC`, userID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var out []domain.SessionFamily
	for rows.Next() {
		var s domain.SessionFamily
		if err := rows.Scan(
			&s.ID, &s.UserID, &s.Client, &s.OAuthClient, &s.DeviceName, &s.Platform,
			&s.OSVersion, &s.Arch, &s.AppVersion, &s.UserAgent, &s.IP, &s.IPRegion,
			&s.CreatedAt, &s.LastSeenAt,
		); err != nil {
			return nil, err
		}
		out = append(out, s)
	}
	return out, rows.Err()
}

// TouchSessionFamily 刷新最近活跃时间。
func TouchSessionFamily(ctx context.Context, q Querier, id uuid.UUID, now time.Time) error {
	_, err := q.Exec(ctx, `UPDATE session_families SET last_seen_at = $2 WHERE id = $1`, id, now)
	return err
}

// UpdateSessionDevice 在 APP 心跳时补全设备信息。
func UpdateSessionDevice(
	ctx context.Context, q Querier, id uuid.UUID, osVersion, arch, appVersion string, now time.Time,
) error {
	_, err := q.Exec(ctx, `
		UPDATE session_families
		   SET os_version = $2, arch = $3, app_version = $4, last_seen_at = $5
		 WHERE id = $1`, id, osVersion, arch, appVersion, now)
	return err
}

// RevokeSessionFamily 撤销单个会话族。
func RevokeSessionFamily(ctx context.Context, q Querier, id uuid.UUID, reason string, now time.Time) error {
	tag, err := q.Exec(ctx, `
		UPDATE session_families
		   SET revoked_at = $2, revoked_reason = $3
		 WHERE id = $1 AND revoked_at IS NULL`, id, now, reason)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}

// RevokeUserSessions 撤销用户的全部会话；except 非 nil 时保留该会话（修改密码保留当前设备）。
func RevokeUserSessions(
	ctx context.Context, q Querier, userID int64, except *uuid.UUID, reason string, now time.Time,
) (int64, error) {
	tag, err := q.Exec(ctx, `
		UPDATE session_families
		   SET revoked_at = $2, revoked_reason = $3
		 WHERE user_id = $1 AND revoked_at IS NULL AND ($4::uuid IS NULL OR id <> $4)`,
		userID, now, reason, except)
	if err != nil {
		return 0, err
	}
	return tag.RowsAffected(), nil
}

// —— refresh token 轮换 ——

// RefreshToken 是一条 refresh token 记录。
type RefreshToken struct {
	ID        uuid.UUID
	FamilyID  uuid.UUID
	UserID    int64
	ExpiresAt time.Time
	UsedAt    *time.Time
	RevokedAt *time.Time
}

// CreateRefreshToken 在会话族下签发一枚 refresh token。
func CreateRefreshToken(
	ctx context.Context, q Querier, familyID uuid.UUID, tokenHash string, expiresAt time.Time,
) (uuid.UUID, error) {
	id := uuid.New()
	_, err := q.Exec(ctx, `
		INSERT INTO refresh_tokens (id, family_id, token_hash, expires_at)
		VALUES ($1, $2, $3, $4)`, id, familyID, tokenHash, expiresAt)
	if err != nil {
		return uuid.Nil, err
	}
	return id, nil
}

// GetRefreshTokenByHash 按摘要读取 refresh token，连带所属会话族的用户与撤销状态。
//
// 这里刻意不过滤 used_at：已使用的令牌被再次出示是重放信号，必须让调用方看到并撤销整族。
func GetRefreshTokenByHash(ctx context.Context, q Querier, tokenHash string) (RefreshToken, error) {
	var t RefreshToken
	err := q.QueryRow(ctx, `
		SELECT rt.id, rt.family_id, sf.user_id, rt.expires_at, rt.used_at,
		       coalesce(rt.revoked_at, sf.revoked_at)
		  FROM refresh_tokens rt
		  JOIN session_families sf ON sf.id = rt.family_id
		 WHERE rt.token_hash = $1`, tokenHash).Scan(
		&t.ID, &t.FamilyID, &t.UserID, &t.ExpiresAt, &t.UsedAt, &t.RevokedAt)
	if err != nil {
		return RefreshToken{}, normalizeErr(err)
	}
	return t, nil
}

// MarkRefreshTokenUsed 标记旧令牌已轮换。返回 ErrNotFound 表示已被并发使用。
func MarkRefreshTokenUsed(
	ctx context.Context, q Querier, id, replacedBy uuid.UUID, now time.Time,
) error {
	tag, err := q.Exec(ctx, `
		UPDATE refresh_tokens
		   SET used_at = $2, replaced_by_id = $3
		 WHERE id = $1 AND used_at IS NULL`, id, now, replacedBy)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}
