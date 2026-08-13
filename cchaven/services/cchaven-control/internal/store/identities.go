package store

import (
	"context"
	"time"
)

// Identity 是「Sub2API 用户 ID ↔ 本地 users.id」的映射行。
type Identity struct {
	Sub2APIUserID string
	UserID        int64
	Email         string
	LinkedAt      time.Time
	LastSeenAt    time.Time
}

// GetIdentityBySub2APIUserID 按 Sub2API 用户 ID 查映射。
func GetIdentityBySub2APIUserID(ctx context.Context, q Querier, sub2apiUserID string) (Identity, error) {
	var out Identity
	err := q.QueryRow(ctx, `
		SELECT sub2api_user_id, user_id, email, linked_at, last_seen_at
		  FROM sub2api_identities
		 WHERE sub2api_user_id = $1`, sub2apiUserID).
		Scan(&out.Sub2APIUserID, &out.UserID, &out.Email, &out.LinkedAt, &out.LastSeenAt)
	if err != nil {
		return Identity{}, normalizeErr(err)
	}
	return out, nil
}

// LinkIdentity 建立映射并同步 users 上的冗余列。
//
// 同一个 Sub2API 用户重复登录会走到这里，因此按 sub2api_user_id 幂等：
// 重复调用只刷新邮箱快照与 last_seen_at。user_id 上的唯一约束保证一个本地账号
// 不会被两个 Sub2API 身份认领——真出现这种数据，宁可报错也不要静默串号。
func LinkIdentity(
	ctx context.Context, q Querier, sub2apiUserID string, userID int64, email string, now time.Time,
) error {
	if _, err := q.Exec(ctx, `
		INSERT INTO sub2api_identities (sub2api_user_id, user_id, email, linked_at, last_seen_at)
		VALUES ($1, $2, $3, $4, $4)
		ON CONFLICT (sub2api_user_id)
		DO UPDATE SET email = EXCLUDED.email, last_seen_at = EXCLUDED.last_seen_at`,
		sub2apiUserID, userID, email, now); err != nil {
		return err
	}

	_, err := q.Exec(ctx,
		`UPDATE users SET sub2api_user_id = $2, updated_at = $3 WHERE id = $1`,
		userID, sub2apiUserID, now)
	return err
}

// TouchIdentity 记录一次身份被使用的时刻，供排查「谁还在用旧映射」。
func TouchIdentity(ctx context.Context, q Querier, sub2apiUserID string, now time.Time) error {
	_, err := q.Exec(ctx,
		`UPDATE sub2api_identities SET last_seen_at = $2 WHERE sub2api_user_id = $1`,
		sub2apiUserID, now)
	return err
}
