package store

import (
	"context"
	"time"
)

// VerificationPurpose 区分注册验证与改邮箱验证。
type VerificationPurpose string

const (
	// PurposeSignup 注册后的邮箱验证。
	PurposeSignup VerificationPurpose = "signup"
	// PurposeEmailChange 账户中心的修改邮箱流程。
	PurposeEmailChange VerificationPurpose = "email_change"
)

// VerificationCode 是一条未消费的验证码记录。
type VerificationCode struct {
	ID           int64
	UserID       int64
	Purpose      VerificationPurpose
	TargetEmail  string
	CodeHash     string
	ExpiresAt    time.Time
	AttemptsUsed int
	MaxAttempts  int
	LastSentAt   time.Time
}

// AttemptsRemaining 返回剩余尝试次数，用于渲染「还剩 {n} 次尝试机会」。
func (c VerificationCode) AttemptsRemaining() int {
	if n := c.MaxAttempts - c.AttemptsUsed; n > 0 {
		return n
	}
	return 0
}

// UpsertVerificationCode 写入验证码；同一用户同一用途已有未消费记录时整体替换。
// 替换而非追加，保证「最新发送的验证码才有效」。
func UpsertVerificationCode(
	ctx context.Context, q Querier,
	userID int64, purpose VerificationPurpose, targetEmail, codeHash string,
	expiresAt, now time.Time,
) error {
	_, err := q.Exec(ctx, `
		INSERT INTO email_verification_codes
		    (user_id, purpose, target_email, code_hash, expires_at, attempts_used, last_sent_at)
		VALUES ($1, $2, $3, $4, $5, 0, $6)
		ON CONFLICT (user_id, purpose) WHERE consumed_at IS NULL
		DO UPDATE SET target_email = EXCLUDED.target_email,
		              code_hash    = EXCLUDED.code_hash,
		              expires_at   = EXCLUDED.expires_at,
		              attempts_used = 0,
		              last_sent_at = EXCLUDED.last_sent_at`,
		userID, purpose, targetEmail, codeHash, expiresAt, now)
	return err
}

// GetActiveVerificationCode 读取未消费的验证码。
func GetActiveVerificationCode(
	ctx context.Context, q Querier, userID int64, purpose VerificationPurpose,
) (VerificationCode, error) {
	var c VerificationCode
	err := q.QueryRow(ctx, `
		SELECT id, user_id, purpose, target_email, code_hash, expires_at,
		       attempts_used, max_attempts, last_sent_at
		  FROM email_verification_codes
		 WHERE user_id = $1 AND purpose = $2 AND consumed_at IS NULL`,
		userID, purpose).Scan(
		&c.ID, &c.UserID, &c.Purpose, &c.TargetEmail, &c.CodeHash, &c.ExpiresAt,
		&c.AttemptsUsed, &c.MaxAttempts, &c.LastSentAt)
	if err != nil {
		return VerificationCode{}, normalizeErr(err)
	}
	return c, nil
}

// IncrementVerificationAttempts 记录一次错误尝试并返回剩余次数。
func IncrementVerificationAttempts(ctx context.Context, q Querier, id int64) (int, error) {
	var remaining int
	err := q.QueryRow(ctx, `
		UPDATE email_verification_codes
		   SET attempts_used = attempts_used + 1
		 WHERE id = $1
		RETURNING greatest(max_attempts - attempts_used, 0)`, id).Scan(&remaining)
	return remaining, normalizeErr(err)
}

// ConsumeVerificationCode 标记验证码已使用。
func ConsumeVerificationCode(ctx context.Context, q Querier, id int64, now time.Time) error {
	_, err := q.Exec(ctx,
		`UPDATE email_verification_codes SET consumed_at = $2 WHERE id = $1 AND consumed_at IS NULL`,
		id, now)
	return err
}

// DeleteVerificationCodes 清除某用途下的全部未消费验证码（用于取消改邮箱流程）。
func DeleteVerificationCodes(ctx context.Context, q Querier, userID int64, purpose VerificationPurpose) error {
	_, err := q.Exec(ctx,
		`DELETE FROM email_verification_codes WHERE user_id = $1 AND purpose = $2 AND consumed_at IS NULL`,
		userID, purpose)
	return err
}

// —— 重设密码令牌 ——

// PasswordResetToken 是一条重设密码令牌记录。
type PasswordResetToken struct {
	ID        int64
	UserID    int64
	ExpiresAt time.Time
}

// CreatePasswordResetToken 写入一次性重设令牌（规范：32 字节随机、20 分钟有效）。
func CreatePasswordResetToken(
	ctx context.Context, q Querier, userID int64, tokenHash, ip string, expiresAt time.Time,
) error {
	_, err := q.Exec(ctx, `
		INSERT INTO password_reset_tokens (user_id, token_hash, expires_at, requested_ip)
		VALUES ($1, $2, $3, $4)`, userID, tokenHash, expiresAt, ip)
	return err
}

// GetValidPasswordResetToken 读取未过期且未使用的令牌。
func GetValidPasswordResetToken(
	ctx context.Context, q Querier, tokenHash string, now time.Time,
) (PasswordResetToken, error) {
	var t PasswordResetToken
	err := q.QueryRow(ctx, `
		SELECT id, user_id, expires_at
		  FROM password_reset_tokens
		 WHERE token_hash = $1 AND consumed_at IS NULL AND expires_at > $2`,
		tokenHash, now).Scan(&t.ID, &t.UserID, &t.ExpiresAt)
	if err != nil {
		return PasswordResetToken{}, normalizeErr(err)
	}
	return t, nil
}

// ConsumePasswordResetToken 标记令牌已使用；返回 ErrNotFound 表示已被并发消费。
func ConsumePasswordResetToken(ctx context.Context, q Querier, id int64, now time.Time) error {
	tag, err := q.Exec(ctx,
		`UPDATE password_reset_tokens SET consumed_at = $2 WHERE id = $1 AND consumed_at IS NULL`,
		id, now)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}

// LatestPasswordResetRequest 返回该用户最近一次申请重设的时间，用于 60 秒冷却判断。
func LatestPasswordResetRequest(ctx context.Context, q Querier, userID int64) (time.Time, error) {
	var at time.Time
	err := q.QueryRow(ctx,
		`SELECT created_at FROM password_reset_tokens
		  WHERE user_id = $1 ORDER BY created_at DESC LIMIT 1`, userID).Scan(&at)
	if err != nil {
		return time.Time{}, normalizeErr(err)
	}
	return at, nil
}
