package service

import (
	"context"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/db"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/domain"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/store"
)

// PublicConfig 是官网与 APP 都会读取的公开配置。
//
// 定价页、账户中心、邀请落地页、下载页的数值与文案一律从这里取，页面不写死
// （交互设计 4.2 / 4.3 / 5.6 / 6.5）。
type PublicConfig struct {
	Pricing struct {
		AmountCents int64  `json:"amount_cents"`
		Currency    string `json:"currency"`
		PeriodUnit  string `json:"period_unit"`
	} `json:"pricing"`
	Invite struct {
		// RewardDays 为 0 时，前端隐藏「每成功邀请 1 人延长 X 天」相关文案。
		RewardDays    int  `json:"reward_days"`
		TrialDays     int  `json:"trial_days"`
		RewardEnabled bool `json:"reward_enabled"`
	} `json:"invite"`
	Releases []store.Release `json:"releases"`
}

// PublicConfig 组装公开配置。
func (s *Service) PublicConfig(ctx context.Context) (PublicConfig, error) {
	cfg, err := store.LoadOpsConfig(ctx, s.Pool)
	if err != nil {
		return PublicConfig{}, err
	}
	releases, err := store.ListCurrentReleases(ctx, s.Pool)
	if err != nil {
		return PublicConfig{}, err
	}

	var out PublicConfig
	out.Pricing.AmountCents = cfg.PricingMonthly.AmountCents
	out.Pricing.Currency = cfg.PricingMonthly.Currency
	out.Pricing.PeriodUnit = "month"
	out.Invite.RewardDays = cfg.InviteRewardDays
	out.Invite.TrialDays = cfg.InviteTrialDays
	out.Invite.RewardEnabled = cfg.RewardEnabled()
	out.Releases = releases
	if out.Releases == nil {
		out.Releases = []store.Release{}
	}
	return out, nil
}

// HeartbeatInput 是 APP 心跳上报。
type HeartbeatInput struct {
	UserID     int64
	SessionID  uuid.UUID
	DeviceID   string
	AppVersion string
	OSVersion  string
	Arch       string
}

// Notice 是下发给 APP 的提醒。
type Notice struct {
	Type     string `json:"type"`
	DaysLeft int    `json:"days_left"`
}

// HeartbeatResult 是心跳响应。
type HeartbeatResult struct {
	Entitlement domain.Entitlement `json:"entitlement"`
	Notices     []Notice           `json:"notices"`
}

// Heartbeat 记录设备与活跃度，并回传订阅快照与到期提醒。
//
// 这条链路同时喂养后台的 DAU、7 日留存、平台分布与 APP 版本分布四项指标。
func (s *Service) Heartbeat(ctx context.Context, in HeartbeatInput) (HeartbeatResult, error) {
	now := s.now()

	err := db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		if err := store.RecordActivity(ctx, tx, in.UserID, now); err != nil {
			return err
		}
		if err := store.TouchLastActive(ctx, tx, in.UserID, now); err != nil {
			return err
		}
		if err := store.UpdateSessionDevice(
			ctx, tx, in.SessionID, in.OSVersion, in.Arch, in.AppVersion, now,
		); err != nil {
			return err
		}
		if in.DeviceID == "" {
			return nil
		}
		return store.UpsertDevice(ctx, tx, in.UserID, in.DeviceID,
			"macos", in.OSVersion, in.Arch, in.AppVersion, now)
	})
	if err != nil {
		return HeartbeatResult{}, err
	}

	entitlement, err := s.Entitlement(ctx, in.UserID)
	if err != nil {
		return HeartbeatResult{}, err
	}

	result := HeartbeatResult{Entitlement: entitlement, Notices: []Notice{}}
	// 剩余 ≤3 天时 APP 展示一次性续费横幅（交互设计 5.6）。
	if entitlement.ExpiringSoon {
		result.Notices = append(result.Notices,
			Notice{Type: "expiring_soon", DaysLeft: entitlement.DaysLeft})
	}
	return result, nil
}

// ExpireDeletedAccounts 处理冷静期已满的注销申请，由后台任务定期调用。
func (s *Service) ExpireDeletedAccounts(ctx context.Context) (int, error) {
	now := s.now()
	cutoff := now.Add(-AccountDeletionGracePeriod)

	rows, err := s.Pool.Query(ctx,
		`SELECT id FROM users WHERE deletion_requested_at IS NOT NULL
		   AND deletion_requested_at <= $1 AND status <> 'disabled'`, cutoff)
	if err != nil {
		return 0, err
	}

	var ids []int64
	for rows.Next() {
		var id int64
		if err := rows.Scan(&id); err != nil {
			rows.Close()
			return 0, err
		}
		ids = append(ids, id)
	}
	rows.Close()
	if err := rows.Err(); err != nil {
		return 0, err
	}

	for _, id := range ids {
		if err := db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
			if err := store.SetUserDisabled(
				ctx, tx, id, true, nil, "用户申请注销，冷静期已满", now,
			); err != nil {
				return err
			}
			_, err := store.RevokeUserSessions(ctx, tx, id, nil, domain.RevokeAccountDeleted, now)
			return err
		}); err != nil {
			return 0, err
		}
	}
	return len(ids), nil
}
