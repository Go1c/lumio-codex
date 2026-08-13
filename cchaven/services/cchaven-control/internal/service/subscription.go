package service

import (
	"context"
	"errors"
	"strconv"
	"time"

	"github.com/jackc/pgx/v5"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/domain"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/security"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/store"
)

// errAlreadySettled 表示该笔时长变更此前已结算过，属于幂等路径而非故障。
var errAlreadySettled = errors.New("service: 该事件已结算")

// Entitlement 返回订阅快照，驱动官网徽标与 APP 账户菜单。
func (s *Service) Entitlement(ctx context.Context, userID int64) (domain.Entitlement, error) {
	sub, err := store.GetSubscription(ctx, s.Pool, userID)
	if err != nil {
		if errors.Is(err, store.ErrNotFound) {
			return domain.Entitlement{Status: domain.EntitlementNone}, nil
		}
		return domain.Entitlement{}, err
	}
	return sub.Snapshot(s.now()), nil
}

// applyDays 在事务中延长订阅并记账。
//
// 调用方必须已经持有相应的行级锁（用户行或归因行），本函数只负责算账与落账。
// 事件表的唯一索引作为最后一道防线：命中时通过 savepoint 回滚该条 INSERT 并
// 返回 errAlreadySettled，不会污染外层事务。
func (s *Service) applyDays(
	ctx context.Context, tx pgx.Tx, userID, days int64, kind *domain.SubscriptionKind,
	event store.SubscriptionEvent,
) (time.Time, error) {
	now := s.now()

	sub, err := store.LockSubscription(ctx, tx, userID)
	if err != nil {
		return time.Time{}, err
	}

	before := sub.ExpiresAt
	after := store.ExtendFrom(sub.ExpiresAt, now, int(days))

	event.UserID = userID
	event.DaysDelta = int(days)
	event.Before = before
	event.After = &after

	if err := insertEventIdempotent(ctx, tx, event); err != nil {
		return time.Time{}, err
	}

	update := store.SubscriptionUpdate{ExpiresAt: &after}
	// 试用不覆盖已生效的付费订阅：kind 只在没有更高级别订阅时才改写。
	if kind != nil {
		if *kind == domain.KindPaid || sub.Kind == nil || *sub.Kind != domain.KindPaid {
			update.Kind = kind
		}
		if *kind == domain.KindTrial {
			update.TrialExpiresAt = &after
		}
	}
	if event.Type == store.EventInviteBonus {
		update.BonusDaysDelta = int(days)
	}

	if err := store.UpdateSubscription(ctx, tx, userID, update, now); err != nil {
		return time.Time{}, err
	}
	return after, nil
}

// insertEventIdempotent 在 savepoint 中写入事件，把唯一约束冲突翻译为 errAlreadySettled。
func insertEventIdempotent(ctx context.Context, tx pgx.Tx, event store.SubscriptionEvent) error {
	savepoint, err := tx.Begin(ctx)
	if err != nil {
		return err
	}

	if err := store.InsertSubscriptionEvent(ctx, savepoint, event); err != nil {
		_ = savepoint.Rollback(ctx)
		if store.IsUniqueViolation(err) {
			return errAlreadySettled
		}
		return err
	}
	return savepoint.Commit(ctx)
}

// ActivationResult 描述首次登录 APP 时的发放结果，供前端渲染祝贺 toast 与拒绝文案。
type ActivationResult struct {
	TrialGranted     bool       `json:"trial_granted"`
	TrialExpiresAt   *time.Time `json:"trial_expires_at,omitempty"`
	TrialDeniedReuse bool       `json:"trial_denied_reuse,omitempty"`
	InviterBonusDays int        `json:"inviter_bonus_days,omitempty"`
}

// ActivationInput 是首次登录 APP 的发放上下文。
type ActivationInput struct {
	UserID   int64
	DeviceID string
	SignupIP string
}

// SettleFirstAppLogin 处理三步闭环的最后一步。
//
// 时点为「首次登录 APP」，确保下载、注册、登录三步全部完成后才发放。
// 同一事务内完成：发放被邀请者试用、标记归因 activated、给邀请者追加奖励天数、入队两封通知。
//
// 任何一项发放失败都不应阻断登录本身——用户拿不到试用是业务结果，不是登录错误。
func (s *Service) SettleFirstAppLogin(
	ctx context.Context, tx pgx.Tx, in ActivationInput,
) (ActivationResult, error) {
	cfg, err := store.LoadOpsConfig(ctx, tx)
	if err != nil {
		return ActivationResult{}, err
	}

	var result ActivationResult

	user, err := store.LockUserForUpdate(ctx, tx, in.UserID)
	if err != nil {
		return ActivationResult{}, err
	}

	// —— 被邀请者试用（每账号一生一次）——
	if user.TrialGrantedAt == nil && cfg.InviteTrialDays > 0 {
		ok, err := store.ClaimTrialFingerprints(ctx, tx, user.ID, fingerprintsOf(in))
		if err != nil {
			return ActivationResult{}, err
		}
		if !ok {
			result.TrialDeniedReuse = true
		} else {
			kind := domain.KindTrial
			expiresAt, err := s.applyDays(ctx, tx, user.ID, int64(cfg.InviteTrialDays), &kind,
				store.SubscriptionEvent{Type: store.EventTrialGranted, Note: "首次登录 APP 发放免费试用"})
			switch {
			case errors.Is(err, errAlreadySettled):
				result.TrialDeniedReuse = true
			case err != nil:
				return ActivationResult{}, err
			default:
				if err := store.MarkTrialGranted(ctx, tx, user.ID, s.now()); err != nil {
					return ActivationResult{}, err
				}
				result.TrialGranted = true
				result.TrialExpiresAt = &expiresAt

				if err := store.EnqueueEmail(ctx, tx, user.Email, store.TemplateTrialGranted,
					map[string]any{
						"days":       cfg.InviteTrialDays,
						"expires_at": expiresAt.Format(time.RFC3339),
					}); err != nil {
					return ActivationResult{}, err
				}
			}
		}
	} else if user.TrialGrantedAt != nil {
		result.TrialDeniedReuse = true
	}

	// —— 邀请者奖励 ——
	bonus, err := s.settleInviterBonus(ctx, tx, user, cfg, result.TrialGranted)
	if err != nil {
		return ActivationResult{}, err
	}
	result.InviterBonusDays = bonus

	return result, nil
}

// settleInviterBonus 结算邀请者奖励，返回实际发放天数。
// 奖励天数由后台配置，配为 0 时闭环照常完成但不发放（前端相应隐藏文案）。
func (s *Service) settleInviterBonus(
	ctx context.Context, tx pgx.Tx, invitee domain.User, cfg store.OpsConfig, trialGranted bool,
) (int, error) {
	attribution, err := store.LockAttributionByInvitee(ctx, tx, invitee.ID)
	if err != nil {
		if errors.Is(err, store.ErrNotFound) {
			return 0, nil // 自然流量注册，无邀请者
		}
		return 0, err
	}
	if attribution.Stage == domain.StageActivated {
		return 0, nil // 已结算过
	}

	now := s.now()
	bonusDays := 0

	if cfg.RewardEnabled() {
		kind := domain.KindPaid
		_, err := s.applyDays(ctx, tx, attribution.InviterUserID, int64(cfg.InviteRewardDays), &kind,
			store.SubscriptionEvent{
				Type:    store.EventInviteBonus,
				RefType: "referral_attribution",
				RefID:   strconv.FormatInt(attribution.ID, 10),
				Note:    "邀请奖励",
			})
		switch {
		case errors.Is(err, errAlreadySettled):
			// 已发放过，仅补齐归因状态。
		case err != nil:
			return 0, err
		default:
			bonusDays = cfg.InviteRewardDays

			inviter, err := store.GetUserByID(ctx, tx, attribution.InviterUserID)
			if err != nil {
				return 0, err
			}
			if err := store.EnqueueEmail(ctx, tx, inviter.Email, store.TemplateInviteRewarded,
				map[string]any{
					"friend": domain.MaskEmail(invitee.Email),
					"days":   bonusDays,
				}); err != nil {
				return 0, err
			}
		}
	}

	if err := store.MarkAttributionActivated(
		ctx, tx, attribution.ID, trialGranted, bonusDays, now,
	); err != nil {
		return 0, err
	}
	return bonusDays, nil
}

// fingerprintsOf 组装试用防滥用指纹。设备 ID 与注册 IP 都只存摘要，不落明文。
func fingerprintsOf(in ActivationInput) []store.TrialFingerprint {
	var fps []store.TrialFingerprint
	if in.DeviceID != "" {
		fps = append(fps, store.TrialFingerprint{
			Kind: "device", ValueHash: security.HashToken(in.DeviceID),
		})
	}
	return fps
}

// CreditPurchase 在订单支付成功后延长订阅。ref 为订单号，保证 webhook 重投不会重复入账。
func (s *Service) CreditPurchase(
	ctx context.Context, tx pgx.Tx, userID int64, orderNo string, months int,
) (time.Time, error) {
	kind := domain.KindPaid
	expiresAt, err := s.applyDays(ctx, tx, userID, int64(months*30), &kind, store.SubscriptionEvent{
		Type:    store.EventPurchase,
		RefType: "order",
		RefID:   orderNo,
		Note:    "包月订阅付款",
	})
	if errors.Is(err, errAlreadySettled) {
		sub, getErr := store.GetSubscription(ctx, tx, userID)
		if getErr != nil {
			return time.Time{}, getErr
		}
		if sub.ExpiresAt != nil {
			return *sub.ExpiresAt, nil
		}
		return time.Time{}, nil
	}
	return expiresAt, err
}

// RevokePurchaseDays 在退款成功后扣回该订单对应的订阅天数。
//
// 只扣回订单本身的时长，不影响试用与邀请奖励累积的天数；
// 扣减后早于当前时刻的，订阅直接判定为已过期。
func (s *Service) RevokePurchaseDays(
	ctx context.Context, tx pgx.Tx, userID int64, orderNo string, months int,
) error {
	_, err := s.applyDays(ctx, tx, userID, int64(-months*30), nil, store.SubscriptionEvent{
		Type:    store.EventRefundRevoke,
		RefType: "order",
		RefID:   orderNo,
		Note:    "退款扣回订阅时长",
	})
	if errors.Is(err, errAlreadySettled) {
		return nil
	}
	return err
}
