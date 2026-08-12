package store

import (
	"context"
	"time"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/domain"
)

const userColumns = `
	id, email, password_hash, display_name, status, email_verified_at, locked_until,
	failed_login_count, registration_source, referred_by_user_id, trial_granted_at,
	deletion_requested_at, disabled_at, coalesce(disabled_reason, ''), last_active_at,
	created_at, updated_at`

func scanUser(row interface{ Scan(...any) error }) (domain.User, error) {
	var u domain.User
	err := row.Scan(
		&u.ID, &u.Email, &u.PasswordHash, &u.DisplayName, &u.Status, &u.EmailVerifiedAt,
		&u.LockedUntil, &u.FailedLoginCount, &u.RegistrationSource, &u.ReferredByUserID,
		&u.TrialGrantedAt, &u.DeletionRequestedAt, &u.DisabledAt, &u.DisabledReason,
		&u.LastActiveAt, &u.CreatedAt, &u.UpdatedAt,
	)
	if err != nil {
		return domain.User{}, normalizeErr(err)
	}
	return u, nil
}

// CreateUserParams 是注册所需的字段。
type CreateUserParams struct {
	Email        string
	PasswordHash string
	Source       domain.RegistrationSource
	ReferredBy   *int64
	SignupIP     string
	UserAgent    string
}

// CreateUser 插入 pending_email 状态的新用户，并同步建立空订阅行。
// 订阅行恒存在可以让后续所有查询免于处理「行不存在」的分支。
func CreateUser(ctx context.Context, q Querier, p CreateUserParams) (domain.User, error) {
	row := q.QueryRow(ctx, `
		INSERT INTO users (email, password_hash, status, registration_source,
		                   referred_by_user_id, signup_ip, signup_user_agent)
		VALUES ($1, $2, 'pending_email', $3, $4, $5, $6)
		RETURNING `+userColumns,
		domain.NormalizeEmail(p.Email), p.PasswordHash, p.Source, p.ReferredBy, p.SignupIP, p.UserAgent)

	user, err := scanUser(row)
	if err != nil {
		return domain.User{}, err
	}

	if _, err := q.Exec(ctx,
		`INSERT INTO subscriptions (user_id) VALUES ($1) ON CONFLICT DO NOTHING`, user.ID); err != nil {
		return domain.User{}, err
	}
	return user, nil
}

// GetUserByID 按主键读取用户。
func GetUserByID(ctx context.Context, q Querier, id int64) (domain.User, error) {
	return scanUser(q.QueryRow(ctx, `SELECT `+userColumns+` FROM users WHERE id = $1`, id))
}

// GetUserByEmail 按邮箱读取用户（大小写不敏感）。
func GetUserByEmail(ctx context.Context, q Querier, email string) (domain.User, error) {
	return scanUser(q.QueryRow(ctx,
		`SELECT `+userColumns+` FROM users WHERE lower(email) = lower($1)`, domain.NormalizeEmail(email)))
}

// LockUserForUpdate 在事务中取行级锁，用于试用发放等需要串行化的路径。
func LockUserForUpdate(ctx context.Context, q Querier, id int64) (domain.User, error) {
	return scanUser(q.QueryRow(ctx, `SELECT `+userColumns+` FROM users WHERE id = $1 FOR UPDATE`, id))
}

// ActivateUser 在邮箱验证成功后把账号置为 active。
func ActivateUser(ctx context.Context, q Querier, id int64, now time.Time) error {
	_, err := q.Exec(ctx, `
		UPDATE users
		   SET status = 'active', email_verified_at = $2, failed_login_count = 0,
		       locked_until = NULL, updated_at = $2
		 WHERE id = $1`, id, now)
	return err
}

// RecordLoginFailure 累加失败次数，达到 threshold 时锁定 lockFor 时长。
// 返回更新后的用户，供调用方判断是否已锁定。
func RecordLoginFailure(
	ctx context.Context, q Querier, id int64, threshold int, lockFor time.Duration, now time.Time,
) (domain.User, error) {
	return scanUser(q.QueryRow(ctx, `
		UPDATE users
		   SET failed_login_count = failed_login_count + 1,
		       locked_until = CASE
		           WHEN failed_login_count + 1 >= $2 THEN $3::timestamptz
		           ELSE locked_until
		       END,
		       updated_at = $4
		 WHERE id = $1
		RETURNING `+userColumns,
		id, threshold, now.Add(lockFor), now))
}

// ClearLoginFailures 在登录成功后清零失败计数与锁定。
func ClearLoginFailures(ctx context.Context, q Querier, id int64, now time.Time) error {
	_, err := q.Exec(ctx, `
		UPDATE users
		   SET failed_login_count = 0, locked_until = NULL, last_active_at = $2, updated_at = $2
		 WHERE id = $1`, id, now)
	return err
}

// UpdateDisplayName 更新显示名称。
func UpdateDisplayName(ctx context.Context, q Querier, id int64, name string, now time.Time) error {
	_, err := q.Exec(ctx,
		`UPDATE users SET display_name = $2, updated_at = $3 WHERE id = $1`, id, name, now)
	return err
}

// UpdatePasswordHash 写入新的口令哈希。
func UpdatePasswordHash(ctx context.Context, q Querier, id int64, hash string, now time.Time) error {
	_, err := q.Exec(ctx,
		`UPDATE users SET password_hash = $2, updated_at = $3 WHERE id = $1`, id, hash, now)
	return err
}

// UpdateEmail 原子切换邮箱。
func UpdateEmail(ctx context.Context, q Querier, id int64, email string, now time.Time) error {
	_, err := q.Exec(ctx, `
		UPDATE users SET email = $2, email_verified_at = $3, updated_at = $3 WHERE id = $1`,
		id, domain.NormalizeEmail(email), now)
	return err
}

// SetUserDisabled 停用或恢复账号。adminID 为 nil 表示系统自动操作。
func SetUserDisabled(
	ctx context.Context, q Querier, id int64, disabled bool, adminID *int64, reason string, now time.Time,
) error {
	if disabled {
		_, err := q.Exec(ctx, `
			UPDATE users
			   SET status = 'disabled', disabled_at = $2, disabled_by_admin_id = $3,
			       disabled_reason = $4, updated_at = $2
			 WHERE id = $1`, id, now, adminID, reason)
		return err
	}

	_, err := q.Exec(ctx, `
		UPDATE users
		   SET status = 'active', disabled_at = NULL, disabled_by_admin_id = NULL,
		       disabled_reason = NULL, updated_at = $2
		 WHERE id = $1`, id, now)
	return err
}

// SetDeletionRequested 设置或撤销注销申请（7 天冷静期）。
func SetDeletionRequested(ctx context.Context, q Querier, id int64, at *time.Time, now time.Time) error {
	_, err := q.Exec(ctx,
		`UPDATE users SET deletion_requested_at = $2, updated_at = $3 WHERE id = $1`, id, at, now)
	return err
}

// MarkTrialGranted 记录试用发放时刻，与 subscription_events 的唯一索引互为双保险。
func MarkTrialGranted(ctx context.Context, q Querier, id int64, now time.Time) error {
	_, err := q.Exec(ctx,
		`UPDATE users SET trial_granted_at = $2, updated_at = $2 WHERE id = $1`, id, now)
	return err
}

// TouchLastActive 更新最近活跃时间，供后台用户列表展示。
func TouchLastActive(ctx context.Context, q Querier, id int64, now time.Time) error {
	_, err := q.Exec(ctx, `UPDATE users SET last_active_at = $2 WHERE id = $1`, id, now)
	return err
}
