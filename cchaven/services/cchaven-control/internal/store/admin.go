package store

import (
	"context"
	"time"

	"github.com/google/uuid"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/domain"
)

// Admin 是管理后台账号。与普通用户完全隔离的独立体系。
type Admin struct {
	ID               int64
	Email            string
	PasswordHash     string
	DisplayName      string
	Role             string
	TOTPSecretEnc    *string
	TOTPEnabledAt    *time.Time
	Status           string
	FailedLoginCount int
	LockedUntil      *time.Time
}

// TOTPEnabled 报告该管理员是否已完成两步验证注册。
func (a Admin) TOTPEnabled() bool { return a.TOTPEnabledAt != nil && a.TOTPSecretEnc != nil }

const adminColumns = `
	id, email, password_hash, display_name, role, totp_secret_enc, totp_enabled_at,
	status, failed_login_count, locked_until`

func scanAdmin(row interface{ Scan(...any) error }) (Admin, error) {
	var a Admin
	err := row.Scan(&a.ID, &a.Email, &a.PasswordHash, &a.DisplayName, &a.Role,
		&a.TOTPSecretEnc, &a.TOTPEnabledAt, &a.Status, &a.FailedLoginCount, &a.LockedUntil)
	if err != nil {
		return Admin{}, normalizeErr(err)
	}
	return a, nil
}

// CreateAdmin 建立管理员账号，由 cmd/admin-bootstrap 调用。
func CreateAdmin(ctx context.Context, q Querier, email, passwordHash, displayName, role string) (Admin, error) {
	return scanAdmin(q.QueryRow(ctx, `
		INSERT INTO admins (email, password_hash, display_name, role)
		VALUES (lower($1), $2, $3, $4)
		RETURNING `+adminColumns, email, passwordHash, displayName, role))
}

// GetAdminByEmail 按邮箱读取管理员。
func GetAdminByEmail(ctx context.Context, q Querier, email string) (Admin, error) {
	return scanAdmin(q.QueryRow(ctx,
		`SELECT `+adminColumns+` FROM admins WHERE email = lower($1)`, email))
}

// GetAdminByID 按主键读取管理员。
func GetAdminByID(ctx context.Context, q Querier, id int64) (Admin, error) {
	return scanAdmin(q.QueryRow(ctx, `SELECT `+adminColumns+` FROM admins WHERE id = $1`, id))
}

// SetAdminTOTP 写入加密后的 TOTP 种子并标记启用。
func SetAdminTOTP(ctx context.Context, q Querier, id int64, secretEnc string, enabledAt *time.Time) error {
	_, err := q.Exec(ctx,
		`UPDATE admins SET totp_secret_enc = $2, totp_enabled_at = $3 WHERE id = $1`,
		id, secretEnc, enabledAt)
	return err
}

// RecordAdminLoginFailure 累加管理员登录失败次数并在达阈值时锁定。
func RecordAdminLoginFailure(
	ctx context.Context, q Querier, id int64, threshold int, lockFor time.Duration, now time.Time,
) error {
	_, err := q.Exec(ctx, `
		UPDATE admins
		   SET failed_login_count = failed_login_count + 1,
		       locked_until = CASE WHEN failed_login_count + 1 >= $2 THEN $3::timestamptz ELSE locked_until END
		 WHERE id = $1`, id, threshold, now.Add(lockFor))
	return err
}

// ClearAdminLoginFailures 登录成功后清零并记录登录时间。
func ClearAdminLoginFailures(ctx context.Context, q Querier, id int64, now time.Time) error {
	_, err := q.Exec(ctx,
		`UPDATE admins SET failed_login_count = 0, locked_until = NULL, last_login_at = $2 WHERE id = $1`,
		id, now)
	return err
}

// AdminSession 是管理端会话。
type AdminSession struct {
	ID        uuid.UUID
	AdminID   int64
	MFAPassed bool
	ExpiresAt time.Time
}

// CreateAdminSession 建立管理端会话。mfaPassed 为 false 时只能用于补做两步验证。
func CreateAdminSession(
	ctx context.Context, q Querier, adminID int64, tokenHash string,
	mfaPassed bool, ip, userAgent string, expiresAt time.Time,
) (uuid.UUID, error) {
	id := uuid.New()
	_, err := q.Exec(ctx, `
		INSERT INTO admin_sessions (id, admin_id, token_hash, mfa_passed, ip, user_agent, expires_at)
		VALUES ($1, $2, $3, $4, $5, $6, $7)`,
		id, adminID, tokenHash, mfaPassed, ip, userAgent, expiresAt)
	if err != nil {
		return uuid.Nil, err
	}
	return id, nil
}

// GetAdminSessionByHash 读取未撤销且未过期的管理端会话。
func GetAdminSessionByHash(ctx context.Context, q Querier, tokenHash string, now time.Time) (AdminSession, error) {
	var s AdminSession
	err := q.QueryRow(ctx, `
		SELECT id, admin_id, mfa_passed, expires_at
		  FROM admin_sessions
		 WHERE token_hash = $1 AND revoked_at IS NULL AND expires_at > $2`,
		tokenHash, now).Scan(&s.ID, &s.AdminID, &s.MFAPassed, &s.ExpiresAt)
	if err != nil {
		return AdminSession{}, normalizeErr(err)
	}
	return s, nil
}

// MarkAdminSessionMFAPassed 在两步验证通过后升级会话。
func MarkAdminSessionMFAPassed(ctx context.Context, q Querier, id uuid.UUID) error {
	_, err := q.Exec(ctx, `UPDATE admin_sessions SET mfa_passed = true WHERE id = $1`, id)
	return err
}

// RevokeAdminSession 撤销管理端会话。
func RevokeAdminSession(ctx context.Context, q Querier, id uuid.UUID, now time.Time) error {
	_, err := q.Exec(ctx, `UPDATE admin_sessions SET revoked_at = $2 WHERE id = $1`, id, now)
	return err
}

// —— 后台用户列表 ——

// AdminUserRow 是后台用户表格的一行。
type AdminUserRow struct {
	ID              int64
	Email           string
	CreatedAt       time.Time
	Source          string
	InviterID       *int64
	Platform        string
	SubState        string
	LastActiveAt    *time.Time
	Status          string
	SubExpiresAt    *time.Time
	SubscriptionKnd *string
}

// ListAdminUsers 分页查询后台用户列表。
//
// subState 把「已禁用」与订阅状态压到同一维度（与原型筛选 chips 一致），
// 禁用优先级最高——被禁用的用户即便仍在订阅期内也显示为已禁用。
func ListAdminUsers(
	ctx context.Context, q Querier, query, subState string, limit, offset int, now time.Time,
) ([]AdminUserRow, int64, error) {
	const base = `
		WITH scoped AS (
			SELECT u.id, u.email, u.created_at, u.registration_source, u.referred_by_user_id,
			       u.last_active_at, u.status, s.kind, s.expires_at,
			       CASE
			           WHEN u.status = 'disabled'                              THEN 'banned'
			           WHEN s.expires_at > $1 AND s.kind = 'paid'              THEN 'sub'
			           WHEN s.expires_at > $1 AND s.kind = 'trial'             THEN 'trial'
			           ELSE 'none'
			       END AS sub_state
			  FROM users u
			  JOIN subscriptions s ON s.user_id = u.id
		)`

	// 空串表示不筛选；显式 ::text 转换避免 PostgreSQL 无法推断参数类型。
	var total int64
	if err := q.QueryRow(ctx, base+`
		SELECT count(*) FROM scoped
		 WHERE ($2::text = '' OR sub_state = $2)
		   AND ($3::text = '' OR email ILIKE '%' || $3 || '%' OR id::text = $3)`,
		now, subState, query).Scan(&total); err != nil {
		return nil, 0, err
	}

	rows, err := q.Query(ctx, base+`
		SELECT s.id, s.email, s.created_at, s.registration_source, s.referred_by_user_id,
		       coalesce(d.platform, ''), coalesce(d.os_version, ''), coalesce(d.arch, ''),
		       s.sub_state, s.last_active_at, s.status, s.expires_at, s.kind
		  FROM scoped s
		  LEFT JOIN LATERAL (
		      SELECT platform, os_version, arch
		        FROM user_devices ud
		       WHERE ud.user_id = s.id
		       ORDER BY ud.last_seen_at DESC
		       LIMIT 1
		  ) d ON true
		 WHERE ($2::text = '' OR s.sub_state = $2)
		   AND ($3::text = '' OR s.email ILIKE '%' || $3 || '%' OR s.id::text = $3)
		 ORDER BY s.created_at DESC
		 LIMIT $4 OFFSET $5`, now, subState, query, limit, offset)
	if err != nil {
		return nil, 0, err
	}
	defer rows.Close()

	var out []AdminUserRow
	for rows.Next() {
		var r AdminUserRow
		var platform, osVersion, arch string
		if err := rows.Scan(&r.ID, &r.Email, &r.CreatedAt, &r.Source, &r.InviterID,
			&platform, &osVersion, &arch, &r.SubState, &r.LastActiveAt, &r.Status,
			&r.SubExpiresAt, &r.SubscriptionKnd); err != nil {
			return nil, 0, err
		}
		r.Platform = domain.FormatPlatform(platform, osVersion, arch)
		out = append(out, r)
	}
	return out, total, rows.Err()
}
