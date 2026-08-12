package store

import (
	"context"
	"time"

	"github.com/google/uuid"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/domain"
)

// ReferralCode 是一名用户的邀请码。
type ReferralCode struct {
	Code       string
	UserID     int64
	DisabledAt *time.Time
}

// CreateReferralCode 为用户绑定邀请码。
func CreateReferralCode(ctx context.Context, q Querier, userID int64, code string) error {
	_, err := q.Exec(ctx,
		`INSERT INTO referral_codes (code, user_id) VALUES ($1, $2)`, code, userID)
	return err
}

// GetReferralCodeByUser 读取用户的邀请码。
func GetReferralCodeByUser(ctx context.Context, q Querier, userID int64) (ReferralCode, error) {
	var c ReferralCode
	err := q.QueryRow(ctx,
		`SELECT code, user_id, disabled_at FROM referral_codes WHERE user_id = $1`, userID).
		Scan(&c.Code, &c.UserID, &c.DisabledAt)
	if err != nil {
		return ReferralCode{}, normalizeErr(err)
	}
	return c, nil
}

// GetReferralCode 按邀请码读取，已停用的码同样返回 ErrNotFound。
func GetReferralCode(ctx context.Context, q Querier, code string) (ReferralCode, error) {
	var c ReferralCode
	err := q.QueryRow(ctx,
		`SELECT code, user_id, disabled_at FROM referral_codes
		  WHERE code = $1 AND disabled_at IS NULL`, code).
		Scan(&c.Code, &c.UserID, &c.DisabledAt)
	if err != nil {
		return ReferralCode{}, normalizeErr(err)
	}
	return c, nil
}

// RecordReferralVisit 记录三步闭环的第一步：邀请链接被访问。
func RecordReferralVisit(
	ctx context.Context, q Querier, code string, visitorID uuid.UUID, ip, userAgent string,
) error {
	_, err := q.Exec(ctx, `
		INSERT INTO referral_visits (code, visitor_id, ip, user_agent)
		VALUES ($1, $2, $3, $4)`, code, visitorID, ip, userAgent)
	return err
}

// CreateAttribution 记录第二步：被邀请者完成注册。
// invitee_user_id 唯一，重复归因会命中唯一约束，保证一人只归因一次。
func CreateAttribution(
	ctx context.Context, q Querier, code string, inviterID, inviteeID int64, visitorID *uuid.UUID,
) error {
	_, err := q.Exec(ctx, `
		INSERT INTO referral_attributions (code, inviter_user_id, invitee_user_id, visitor_id, stage)
		VALUES ($1, $2, $3, $4, 'registered')`, code, inviterID, inviteeID, visitorID)
	return err
}

// GetAttributionByInvitee 读取被邀请者的归因记录。
func GetAttributionByInvitee(ctx context.Context, q Querier, inviteeID int64) (domain.ReferralAttribution, error) {
	var a domain.ReferralAttribution
	err := q.QueryRow(ctx, `
		SELECT id, code, inviter_user_id, invitee_user_id, stage, registered_at,
		       activated_at, trial_granted, inviter_bonus_days, inviter_bonus_granted_at
		  FROM referral_attributions WHERE invitee_user_id = $1`, inviteeID).Scan(
		&a.ID, &a.Code, &a.InviterUserID, &a.InviteeUserID, &a.Stage, &a.RegisteredAt,
		&a.ActivatedAt, &a.TrialGranted, &a.InviterBonusDays, &a.InviterBonusGrantedAt)
	if err != nil {
		return domain.ReferralAttribution{}, normalizeErr(err)
	}
	return a, nil
}

// LockAttributionByInvitee 取行级锁后读取归因记录，用于闭环结算。
func LockAttributionByInvitee(ctx context.Context, q Querier, inviteeID int64) (domain.ReferralAttribution, error) {
	var a domain.ReferralAttribution
	err := q.QueryRow(ctx, `
		SELECT id, code, inviter_user_id, invitee_user_id, stage, registered_at,
		       activated_at, trial_granted, inviter_bonus_days, inviter_bonus_granted_at
		  FROM referral_attributions WHERE invitee_user_id = $1 FOR UPDATE`, inviteeID).Scan(
		&a.ID, &a.Code, &a.InviterUserID, &a.InviteeUserID, &a.Stage, &a.RegisteredAt,
		&a.ActivatedAt, &a.TrialGranted, &a.InviterBonusDays, &a.InviterBonusGrantedAt)
	if err != nil {
		return domain.ReferralAttribution{}, normalizeErr(err)
	}
	return a, nil
}

// MarkAttributionActivated 记录第三步：被邀请者首次登录 APP，闭环完成。
func MarkAttributionActivated(
	ctx context.Context, q Querier, id int64, trialGranted bool, bonusDays int, now time.Time,
) error {
	var bonusAt *time.Time
	if bonusDays > 0 {
		bonusAt = &now
	}
	_, err := q.Exec(ctx, `
		UPDATE referral_attributions
		   SET stage = 'activated', activated_at = $2, trial_granted = $3,
		       inviter_bonus_days = $4, inviter_bonus_granted_at = $5
		 WHERE id = $1`, id, now, trialGranted, bonusDays, bonusAt)
	return err
}

// ReferralProgressItem 是账户中心邀请进度列表中的一项。
type ReferralProgressItem struct {
	EmailMasked string
	Stage       domain.ReferralStage
	BonusDays   int
	At          time.Time
}

// ListReferralProgress 列出邀请者名下的邀请进度。
func ListReferralProgress(ctx context.Context, q Querier, inviterID int64) ([]ReferralProgressItem, error) {
	rows, err := q.Query(ctx, `
		SELECT u.email, ra.stage, ra.inviter_bonus_days,
		       coalesce(ra.activated_at, ra.registered_at)
		  FROM referral_attributions ra
		  JOIN users u ON u.id = ra.invitee_user_id
		 WHERE ra.inviter_user_id = $1
		 ORDER BY ra.created_at DESC`, inviterID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var out []ReferralProgressItem
	for rows.Next() {
		var email string
		var item ReferralProgressItem
		if err := rows.Scan(&email, &item.Stage, &item.BonusDays, &item.At); err != nil {
			return nil, err
		}
		item.EmailMasked = domain.MaskEmail(email)
		out = append(out, item)
	}
	return out, rows.Err()
}

// ReferralSummary 是账户中心邀请区块的汇总数字。
type ReferralSummary struct {
	ActivatedCount int
	TotalBonusDays int
}

// GetReferralSummary 统计「已成功邀请 n 人 · 订阅共延长 m 天」。
func GetReferralSummary(ctx context.Context, q Querier, inviterID int64) (ReferralSummary, error) {
	var s ReferralSummary
	err := q.QueryRow(ctx, `
		SELECT count(*) FILTER (WHERE stage = 'activated'),
		       coalesce(sum(inviter_bonus_days), 0)
		  FROM referral_attributions
		 WHERE inviter_user_id = $1`, inviterID).Scan(&s.ActivatedCount, &s.TotalBonusDays)
	return s, err
}
