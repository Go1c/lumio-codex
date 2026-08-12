package store

import (
	"context"
	"time"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/domain"
)

// 订阅时长变更事件类型。
const (
	EventTrialGranted = "trial_granted"
	EventInviteBonus  = "invite_bonus"
	EventPurchase     = "purchase"
	EventRefundRevoke = "refund_revoke"
	EventAdminAdjust  = "admin_adjust"
)

// GetSubscription 读取订阅行。注册时已建行，正常情况下必然存在。
func GetSubscription(ctx context.Context, q Querier, userID int64) (domain.Subscription, error) {
	var s domain.Subscription
	err := q.QueryRow(ctx, `
		SELECT user_id, kind, expires_at, trial_expires_at, bonus_days_total, updated_at
		  FROM subscriptions WHERE user_id = $1`, userID).Scan(
		&s.UserID, &s.Kind, &s.ExpiresAt, &s.TrialExpiresAt, &s.BonusDaysTotal, &s.UpdatedAt)
	if err != nil {
		return domain.Subscription{}, normalizeErr(err)
	}
	return s, nil
}

// LockSubscription 取行级锁，保证并发的时长变更串行执行。
func LockSubscription(ctx context.Context, q Querier, userID int64) (domain.Subscription, error) {
	var s domain.Subscription
	err := q.QueryRow(ctx, `
		SELECT user_id, kind, expires_at, trial_expires_at, bonus_days_total, updated_at
		  FROM subscriptions WHERE user_id = $1 FOR UPDATE`, userID).Scan(
		&s.UserID, &s.Kind, &s.ExpiresAt, &s.TrialExpiresAt, &s.BonusDaysTotal, &s.UpdatedAt)
	if err != nil {
		return domain.Subscription{}, normalizeErr(err)
	}
	return s, nil
}

// SubscriptionUpdate 描述一次订阅行写入。
type SubscriptionUpdate struct {
	Kind           *domain.SubscriptionKind
	ExpiresAt      *time.Time
	TrialExpiresAt *time.Time
	BonusDaysDelta int
}

// UpdateSubscription 写入订阅行。BonusDaysDelta 累加到 bonus_days_total。
func UpdateSubscription(
	ctx context.Context, q Querier, userID int64, u SubscriptionUpdate, now time.Time,
) error {
	_, err := q.Exec(ctx, `
		UPDATE subscriptions
		   SET kind             = coalesce($2, kind),
		       expires_at       = coalesce($3, expires_at),
		       trial_expires_at = coalesce($4, trial_expires_at),
		       bonus_days_total = bonus_days_total + $5,
		       updated_at       = $6
		 WHERE user_id = $1`,
		userID, u.Kind, u.ExpiresAt, u.TrialExpiresAt, u.BonusDaysDelta, now)
	return err
}

// SubscriptionEvent 是一条时长变更记录。
type SubscriptionEvent struct {
	UserID    int64
	Type      string
	DaysDelta int
	Before    *time.Time
	After     *time.Time
	RefType   string
	RefID     string
	Note      string
}

// InsertSubscriptionEvent 写入时长变更总账。
//
// 唯一索引在这里承担业务不变量：
//   - trial_granted 每个 user_id 只允许一条 → 「每个账号只可享用一次免费试用」
//   - (type, ref_type, ref_id) 唯一 → 同一次邀请/同一笔订单只结算一次（webhook 可重投）
//
// 命中冲突时返回的错误可用 IsUniqueViolation 判定，属于预期路径。
func InsertSubscriptionEvent(ctx context.Context, q Querier, e SubscriptionEvent) error {
	_, err := q.Exec(ctx, `
		INSERT INTO subscription_events
		    (user_id, type, days_delta, expires_at_before, expires_at_after, ref_type, ref_id, note)
		VALUES ($1, $2, $3, $4, $5, nullif($6, ''), nullif($7, ''), $8)`,
		e.UserID, e.Type, e.DaysDelta, e.Before, e.After, e.RefType, e.RefID, e.Note)
	return err
}

// HasSubscriptionEvent 报告某类事件是否已存在（用于发放前的快速判断）。
func HasSubscriptionEvent(ctx context.Context, q Querier, userID int64, eventType string) (bool, error) {
	var exists bool
	err := q.QueryRow(ctx,
		`SELECT EXISTS (SELECT 1 FROM subscription_events WHERE user_id = $1 AND type = $2)`,
		userID, eventType).Scan(&exists)
	return exists, err
}

// ExtendFrom 计算延长后的到期时间：已在有效期内则顺延，已过期或从未订阅则从此刻起算。
func ExtendFrom(current *time.Time, now time.Time, days int) time.Time {
	base := now
	if current != nil && current.After(now) {
		base = *current
	}
	return base.AddDate(0, 0, days)
}

// —— 防滥用指纹 ——

// TrialFingerprint 是一条试用防滥用指纹。
type TrialFingerprint struct {
	Kind      string
	ValueHash string
}

// ClaimTrialFingerprints 尝试占用一组指纹。任一指纹已被其他账号占用即返回 false，
// 调用方据此拒绝发放试用并返回固定文案「每个账号只可享用一次免费试用。」
func ClaimTrialFingerprints(
	ctx context.Context, q Querier, userID int64, fps []TrialFingerprint,
) (bool, error) {
	for _, fp := range fps {
		if fp.ValueHash == "" {
			continue
		}
		tag, err := q.Exec(ctx, `
			INSERT INTO trial_fingerprints (kind, value_hash, user_id)
			VALUES ($1, $2, $3)
			ON CONFLICT (kind, value_hash) DO NOTHING`, fp.Kind, fp.ValueHash, userID)
		if err != nil {
			return false, err
		}
		if tag.RowsAffected() == 0 {
			// 指纹已被占用；若占用者就是本人则视为重试，否则判定为重复领取。
			var owner int64
			err := q.QueryRow(ctx,
				`SELECT user_id FROM trial_fingerprints WHERE kind = $1 AND value_hash = $2`,
				fp.Kind, fp.ValueHash).Scan(&owner)
			if err != nil {
				return false, normalizeErr(err)
			}
			if owner != userID {
				return false, nil
			}
		}
	}
	return true, nil
}
