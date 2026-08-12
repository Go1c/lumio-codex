package service

import (
	"context"
	"errors"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/apperr"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/db"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/domain"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/security"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/store"
)

// UserView 是账号资料的对外表示。
type UserView struct {
	ID                  string     `json:"id"`
	Email               string     `json:"email"`
	DisplayName         string     `json:"display_name"`
	CreatedAt           time.Time  `json:"created_at"`
	DeletionRequestedAt *time.Time `json:"deletion_requested_at,omitempty"`
	DeletionEffectiveAt *time.Time `json:"deletion_effective_at,omitempty"`
}

// ViewUser 把领域实体转换为对外表示。
func ViewUser(u domain.User) UserView {
	v := UserView{
		ID:                  u.DisplayID(),
		Email:               u.Email,
		DisplayName:         u.DisplayName,
		CreatedAt:           u.CreatedAt,
		DeletionRequestedAt: u.DeletionRequestedAt,
	}
	if u.DeletionRequestedAt != nil {
		effective := u.DeletionRequestedAt.Add(AccountDeletionGracePeriod)
		v.DeletionEffectiveAt = &effective
	}
	return v
}

// UpdateProfile 修改显示名称。
func (s *Service) UpdateProfile(ctx context.Context, userID int64, displayName string) (UserView, error) {
	name := strings.TrimSpace(displayName)
	if len([]rune(name)) > 40 {
		return UserView{}, apperr.InvalidParams()
	}
	if err := store.UpdateDisplayName(ctx, s.Pool, userID, name, s.now()); err != nil {
		return UserView{}, err
	}

	user, err := store.GetUserByID(ctx, s.Pool, userID)
	if err != nil {
		return UserView{}, err
	}
	return ViewUser(user), nil
}

// ChangePassword 修改密码并撤销其他设备的会话，保留当前会话。
func (s *Service) ChangePassword(
	ctx context.Context, userID int64, current uuid.UUID, currentPassword, newPassword string,
) error {
	if !security.ValidatePassword(newPassword) {
		return apperr.PasswordTooWeak()
	}

	now := s.now()
	return db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		user, err := store.GetUserByID(ctx, tx, userID)
		if err != nil {
			return err
		}
		if err := s.Hasher.Verify(currentPassword, user.PasswordHash); err != nil {
			if errors.Is(err, security.ErrPasswordMismatch) {
				return apperr.CurrentPasswordInvalid()
			}
			return err
		}

		hash, err := s.Hasher.Hash(newPassword)
		if err != nil {
			return err
		}
		if err := store.UpdatePasswordHash(ctx, tx, userID, hash, now); err != nil {
			return err
		}

		_, err = store.RevokeUserSessions(ctx, tx, userID, &current, domain.RevokePasswordChange, now)
		return err
	})
}

// RequestEmailChange 向新邮箱发送验证码。原邮箱在切换完成后才会收到通知。
func (s *Service) RequestEmailChange(ctx context.Context, userID int64, newEmail string) (string, error) {
	email := domain.NormalizeEmail(newEmail)
	if !validEmail(email) {
		return "", apperr.EmailInvalid()
	}

	code, err := security.NumericCode(VerificationCodeLength)
	if err != nil {
		return "", err
	}

	now := s.now()
	err = db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		if existing, err := store.GetUserByEmail(ctx, tx, email); err == nil {
			if existing.ID != userID {
				return apperr.EmailTaken()
			}
			return apperr.InvalidParams()
		} else if !errors.Is(err, store.ErrNotFound) {
			return err
		}

		if err := store.UpsertVerificationCode(ctx, tx, userID, store.PurposeEmailChange, email,
			security.HashCode(code, s.Cfg.CodePepper), now.Add(VerificationCodeTTL), now); err != nil {
			return err
		}

		return store.EnqueueEmail(ctx, tx, email, store.TemplateEmailChange, map[string]any{
			"code":       code,
			"expires_in": int(VerificationCodeTTL.Minutes()),
		})
	})

	return s.devCode(code), err
}

// ConfirmEmailChange 校验验证码并原子切换邮箱，同时通知原邮箱。
func (s *Service) ConfirmEmailChange(ctx context.Context, userID int64, code string) (UserView, error) {
	now := s.now()
	var view UserView

	err := db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		record, err := store.GetActiveVerificationCode(ctx, tx, userID, store.PurposeEmailChange)
		if err != nil {
			if errors.Is(err, store.ErrNotFound) {
				return apperr.CodeExpired()
			}
			return err
		}
		if !record.ExpiresAt.After(now) || record.AttemptsRemaining() == 0 {
			return apperr.CodeExpired()
		}

		if record.CodeHash != security.HashCode(code, s.Cfg.CodePepper) {
			remaining, err := store.IncrementVerificationAttempts(ctx, tx, record.ID)
			if err != nil {
				return err
			}
			if remaining == 0 {
				return apperr.CodeExpired()
			}
			return apperr.CodeInvalid(remaining)
		}

		previous, err := store.GetUserByID(ctx, tx, userID)
		if err != nil {
			return err
		}

		if err := store.ConsumeVerificationCode(ctx, tx, record.ID, now); err != nil {
			return err
		}
		if err := store.UpdateEmail(ctx, tx, userID, record.TargetEmail, now); err != nil {
			if store.IsUniqueViolation(err) {
				return apperr.EmailTaken()
			}
			return err
		}
		if err := store.EnqueueEmail(ctx, tx, previous.Email, store.TemplateEmailChanged,
			map[string]any{"new_email": domain.MaskEmail(record.TargetEmail)}); err != nil {
			return err
		}

		updated, err := store.GetUserByID(ctx, tx, userID)
		if err != nil {
			return err
		}
		view = ViewUser(updated)
		return nil
	})

	return view, err
}

// CancelEmailChange 取消进行中的改邮箱流程。
func (s *Service) CancelEmailChange(ctx context.Context, userID int64) error {
	return store.DeleteVerificationCodes(ctx, s.Pool, userID, store.PurposeEmailChange)
}

// RequestAccountDeletion 申请注销，7 天冷静期内可撤销。
func (s *Service) RequestAccountDeletion(ctx context.Context, userID int64) (time.Time, error) {
	now := s.now()
	effective := now.Add(AccountDeletionGracePeriod)

	err := db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		user, err := store.GetUserByID(ctx, tx, userID)
		if err != nil {
			return err
		}
		if err := store.SetDeletionRequested(ctx, tx, userID, &now, now); err != nil {
			return err
		}
		return store.EnqueueEmail(ctx, tx, user.Email, store.TemplateDeletionNotice,
			map[string]any{"effective_at": effective.Format(time.RFC3339)})
	})

	return effective, err
}

// CancelAccountDeletion 撤销注销申请。
func (s *Service) CancelAccountDeletion(ctx context.Context, userID int64) error {
	return store.SetDeletionRequested(ctx, s.Pool, userID, nil, s.now())
}

// ReferralItem 是邀请进度列表中的一项。
type ReferralItem struct {
	EmailMasked string    `json:"email_masked"`
	Status      string    `json:"status"`
	BonusDays   int       `json:"bonus_days"`
	At          time.Time `json:"at"`
}

// ReferralOverview 是账户中心「邀请好友」区块的全部数据。
//
// RewardDays 为 0 时前端隐藏奖励相关文案，数值一律以此为准，不在前端写死。
type ReferralOverview struct {
	Code           string         `json:"code"`
	Link           string         `json:"link"`
	RewardDays     int            `json:"reward_days"`
	TrialDays      int            `json:"trial_days"`
	InvitedCount   int            `json:"invited_count"`
	TotalBonusDays int            `json:"total_bonus_days"`
	Items          []ReferralItem `json:"items"`
}

// ReferralOverviewFor 汇总邀请码、奖励规则与邀请进度。
func (s *Service) ReferralOverviewFor(ctx context.Context, userID int64) (ReferralOverview, error) {
	cfg, err := store.LoadOpsConfig(ctx, s.Pool)
	if err != nil {
		return ReferralOverview{}, err
	}

	code, err := store.GetReferralCodeByUser(ctx, s.Pool, userID)
	if err != nil {
		if !errors.Is(err, store.ErrNotFound) {
			return ReferralOverview{}, err
		}
		// 老账号可能尚未分配邀请码，按需补发一次。
		if err := db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
			return s.assignReferralCode(ctx, tx, userID)
		}); err != nil {
			return ReferralOverview{}, err
		}
		if code, err = store.GetReferralCodeByUser(ctx, s.Pool, userID); err != nil {
			return ReferralOverview{}, err
		}
	}

	summary, err := store.GetReferralSummary(ctx, s.Pool, userID)
	if err != nil {
		return ReferralOverview{}, err
	}
	progress, err := store.ListReferralProgress(ctx, s.Pool, userID)
	if err != nil {
		return ReferralOverview{}, err
	}

	items := make([]ReferralItem, 0, len(progress))
	for _, p := range progress {
		items = append(items, ReferralItem{
			EmailMasked: p.EmailMasked,
			Status:      string(p.Stage),
			BonusDays:   p.BonusDays,
			At:          p.At,
		})
	}

	return ReferralOverview{
		Code:           code.Code,
		Link:           s.Cfg.PublicURL + "/i/" + code.Code,
		RewardDays:     cfg.InviteRewardDays,
		TrialDays:      cfg.InviteTrialDays,
		InvitedCount:   summary.ActivatedCount,
		TotalBonusDays: summary.TotalBonusDays,
		Items:          items,
	}, nil
}

// InviteLanding 是邀请落地页 /i/{code} 的数据。
// Valid 为 false 时前端展示「此邀请链接已失效」，但仍保留正常注册入口，不阻断转化。
type InviteLanding struct {
	Valid     bool   `json:"valid"`
	Code      string `json:"code"`
	Inviter   string `json:"inviter,omitempty"`
	TrialDays int    `json:"trial_days"`
}

// ResolveInvite 解析邀请码并记录一次访问（三步闭环的第一步）。
func (s *Service) ResolveInvite(
	ctx context.Context, code string, visitorID uuid.UUID, ip, userAgent string,
) (InviteLanding, error) {
	landing, err := s.lookupInvite(ctx, code)
	if err != nil {
		return InviteLanding{}, err
	}
	// 失效邀请码不记访问：闭环的第一步统计的是「有效邀请链接被打开」。
	if !landing.Valid {
		return landing, nil
	}

	if err := store.RecordReferralVisit(ctx, s.Pool, code, visitorID, ip, userAgent); err != nil {
		return InviteLanding{}, err
	}
	return landing, nil
}

// lookupInvite 解析邀请码并组装落地数据，不产生任何副作用。
// 落地页与首页横幅共用它，保证 valid / inviter / trial_days 三个字段的口径只有一处。
func (s *Service) lookupInvite(ctx context.Context, code string) (InviteLanding, error) {
	cfg, err := store.LoadOpsConfig(ctx, s.Pool)
	if err != nil {
		return InviteLanding{}, err
	}

	landing := InviteLanding{Code: code, TrialDays: cfg.InviteTrialDays}

	// 已停用的邀请码在 store 层就等同于不存在。
	rc, err := store.GetReferralCode(ctx, s.Pool, code)
	if err != nil {
		if errors.Is(err, store.ErrNotFound) {
			return landing, nil
		}
		return InviteLanding{}, err
	}

	inviter, err := store.GetUserByID(ctx, s.Pool, rc.UserID)
	if err != nil {
		return InviteLanding{}, err
	}

	landing.Valid = true
	landing.Inviter = inviterDisplay(inviter)
	return landing, nil
}

// InviteAttribution 是首页邀请横幅的数据源。
//
// 归因的唯一依据是 HttpOnly 的 cch_ref cookie，前端 JS 读不到它，
// 因此横幅不能靠前端自己缓存的副本来决定显示与否：那份副本不会随 cookie 过期，
// 也不会因为邀请码被停用而失效，会让用户看到「注册即享首月免费」却拿不到。
//
// Attributed 为 false 时不下发 Inviter 与 TrialDays——此时没有任何可承诺的东西。
// 同理，运营把 invite.trial_days 配成 0 时 trial_days 也不出现，前端本就不该显示天数。
type InviteAttribution struct {
	Attributed bool   `json:"attributed"`
	Inviter    string `json:"inviter,omitempty"`
	TrialDays  int    `json:"trial_days,omitempty"`
}

// CurrentInvite 判断当前访客是否仍处于有效的邀请归因下。
//
// code 取自 cch_ref cookie，为空表示访客从未打开过邀请链接。
// 这里刻意不记 referral_visits：那是「邀请链接被访问」这一事件，
// 而首页横幅每次渲染都会调用本接口，记进去会把访问量刷成假数据。
func (s *Service) CurrentInvite(ctx context.Context, code string) (InviteAttribution, error) {
	if code == "" {
		return InviteAttribution{}, nil
	}

	landing, err := s.lookupInvite(ctx, code)
	if err != nil {
		return InviteAttribution{}, err
	}
	if !landing.Valid {
		return InviteAttribution{}, nil
	}

	return InviteAttribution{
		Attributed: true,
		Inviter:    landing.Inviter,
		TrialDays:  landing.TrialDays,
	}, nil
}

// inviterDisplay 优先展示昵称，没有昵称时退回邮箱本地部分，避免暴露完整邮箱。
func inviterDisplay(u domain.User) string {
	if name := strings.TrimSpace(u.DisplayName); name != "" {
		return name
	}
	local, _, _ := strings.Cut(u.Email, "@")
	return local
}
