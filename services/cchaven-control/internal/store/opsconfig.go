package store

import (
	"context"
	"encoding/json"
	"time"
)

// 运营配置键。前台价格与邀请文案一律从这里下发，页面不写死（交互设计 7.4 / 6.5）。
const (
	// ConfigInviteRewardDays 邀请者每成功邀请 1 人延长的天数；0 表示关闭该奖励。
	ConfigInviteRewardDays = "invite.reward_days"
	// ConfigInviteTrialDays 被邀请者免费试用时长。
	ConfigInviteTrialDays = "invite.trial_days"
	// ConfigPricingMonthly 包月价格。
	ConfigPricingMonthly = "pricing.monthly"
)

// Price 是包月价格配置。
type Price struct {
	AmountCents int64  `json:"amount_cents"`
	Currency    string `json:"currency"`
}

// OpsConfig 是运营配置的强类型快照。
type OpsConfig struct {
	InviteRewardDays int   `json:"invite_reward_days"`
	InviteTrialDays  int   `json:"invite_trial_days"`
	PricingMonthly   Price `json:"pricing_monthly"`
}

// RewardEnabled 报告邀请者奖励是否开启。配为 0 时前端隐藏相关文案。
func (c OpsConfig) RewardEnabled() bool { return c.InviteRewardDays > 0 }

// 缺省值：数据库缺键时兜底，保证服务不会因为配置缺失而不可用。
var defaultConfig = OpsConfig{
	InviteRewardDays: 7,
	InviteTrialDays:  30,
	PricingMonthly:   Price{AmountCents: 6800, Currency: "CNY"},
}

// LoadOpsConfig 读取全部运营配置。
func LoadOpsConfig(ctx context.Context, q Querier) (OpsConfig, error) {
	rows, err := q.Query(ctx, `SELECT key, value FROM ops_configs`)
	if err != nil {
		return OpsConfig{}, err
	}
	defer rows.Close()

	cfg := defaultConfig
	for rows.Next() {
		var key string
		var raw []byte
		if err := rows.Scan(&key, &raw); err != nil {
			return OpsConfig{}, err
		}
		switch key {
		case ConfigInviteRewardDays:
			_ = json.Unmarshal(raw, &cfg.InviteRewardDays)
		case ConfigInviteTrialDays:
			_ = json.Unmarshal(raw, &cfg.InviteTrialDays)
		case ConfigPricingMonthly:
			_ = json.Unmarshal(raw, &cfg.PricingMonthly)
		}
	}
	return cfg, rows.Err()
}

// SetOpsConfig 写入一项配置并返回旧值，供审计日志记录前后值。
func SetOpsConfig(
	ctx context.Context, q Querier, key string, value any, adminID int64, now time.Time,
) (json.RawMessage, error) {
	encoded, err := json.Marshal(value)
	if err != nil {
		return nil, err
	}

	var previous json.RawMessage
	// 旧值可能不存在，此处允许 ErrNotFound。
	if err := q.QueryRow(ctx, `SELECT value FROM ops_configs WHERE key = $1`, key).
		Scan(&previous); err != nil && normalizeErr(err) != ErrNotFound {
		return nil, err
	}

	if _, err := q.Exec(ctx, `
		INSERT INTO ops_configs (key, value, updated_at, updated_by_admin_id)
		VALUES ($1, $2, $3, $4)
		ON CONFLICT (key) DO UPDATE
		    SET value = EXCLUDED.value,
		        updated_at = EXCLUDED.updated_at,
		        updated_by_admin_id = EXCLUDED.updated_by_admin_id`,
		key, encoded, now, adminID); err != nil {
		return nil, err
	}
	return previous, nil
}

// —— 版本发布 ——

// Release 是一条发布记录，供下载页展示。
type Release struct {
	Version     string    `json:"version"`
	Arch        string    `json:"arch"`
	DownloadURL string    `json:"download_url"`
	MinOS       string    `json:"min_os"`
	ReleasedAt  time.Time `json:"released_at"`
}

// ListCurrentReleases 列出当前版本的各架构下载项。
func ListCurrentReleases(ctx context.Context, q Querier) ([]Release, error) {
	rows, err := q.Query(ctx, `
		SELECT version, arch, download_url, min_os, released_at
		  FROM app_releases
		 WHERE is_current AND channel = 'stable'
		 ORDER BY arch`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var out []Release
	for rows.Next() {
		var r Release
		if err := rows.Scan(&r.Version, &r.Arch, &r.DownloadURL, &r.MinOS, &r.ReleasedAt); err != nil {
			return nil, err
		}
		out = append(out, r)
	}
	return out, rows.Err()
}
