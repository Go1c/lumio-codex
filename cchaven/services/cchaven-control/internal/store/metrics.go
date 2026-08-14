package store

import (
	"context"
	"time"
)

// DailyCount 是「某天 / 某个分类」的一个计数点。
type DailyCount struct {
	Day   time.Time `json:"day"`
	Count int64     `json:"count"`
}

// Bucket 是分布图中的一段。
type Bucket struct {
	Label string `json:"label"`
	Count int64  `json:"count"`
}

// CountActiveUsers 统计某一天的日活。
func CountActiveUsers(ctx context.Context, q Querier, day time.Time) (int64, error) {
	var n int64
	err := q.QueryRow(ctx,
		`SELECT count(*) FROM user_activity_days WHERE day = $1::date`, day).Scan(&n)
	return n, err
}

// DailyActiveSeries 返回最近 days 天的日活序列，无数据的日期补 0，保证柱状图不断档。
func DailyActiveSeries(ctx context.Context, q Querier, until time.Time, days int) ([]DailyCount, error) {
	rows, err := q.Query(ctx, `
		SELECT d::date, coalesce(count(a.user_id), 0)
		  FROM generate_series($1::date - ($2::int - 1), $1::date, interval '1 day') AS d
		  LEFT JOIN user_activity_days a ON a.day = d::date
		 GROUP BY d ORDER BY d`, until, days)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var out []DailyCount
	for rows.Next() {
		var p DailyCount
		if err := rows.Scan(&p.Day, &p.Count); err != nil {
			return nil, err
		}
		out = append(out, p)
	}
	return out, rows.Err()
}

// SignupCounts 统计某时间窗内的新增注册数与其中经邀请的人数。
func SignupCounts(ctx context.Context, q Querier, from, to time.Time) (total, invited int64, err error) {
	err = q.QueryRow(ctx, `
		SELECT count(*), count(*) FILTER (WHERE registration_source = 'invite')
		  FROM users WHERE created_at >= $1 AND created_at < $2`, from, to).Scan(&total, &invited)
	return total, invited, err
}

// SubscriberCounts 统计当前付费订阅人数与试用中人数。
func SubscriberCounts(ctx context.Context, q Querier, now time.Time) (paid, trialing int64, err error) {
	err = q.QueryRow(ctx, `
		SELECT count(*) FILTER (WHERE kind = 'paid'  AND expires_at > $1),
		       count(*) FILTER (WHERE kind = 'trial' AND expires_at > $1)
		  FROM subscriptions`, now).Scan(&paid, &trialing)
	return paid, trialing, err
}

// TrialConversionRate 计算近 days 天内开通试用的用户中，最终产生过已支付订单的比例。
// 分母为 0 时返回 ok=false，由上层渲染为「—」而不是 0%，避免误读。
func TrialConversionRate(ctx context.Context, q Querier, now time.Time, days int) (rate float64, ok bool, err error) {
	var cohort, converted int64
	err = q.QueryRow(ctx, `
		WITH cohort AS (
		    SELECT user_id FROM subscription_events
		     WHERE type = 'trial_granted' AND created_at >= $1::timestamptz - make_interval(days => $2::int)
		)
		SELECT (SELECT count(*) FROM cohort),
		       (SELECT count(DISTINCT c.user_id)
		          FROM cohort c JOIN orders o ON o.user_id = c.user_id AND o.status = 'paid')`,
		now, days).Scan(&cohort, &converted)
	if err != nil || cohort == 0 {
		return 0, false, err
	}
	return float64(converted) / float64(cohort), true, nil
}

// RetentionD7 计算 7 日留存：注册于 asOf-7 天当日的用户中，在 asOf 当日仍有活跃的比例。
// 队列为空返回 ok=false。
func RetentionD7(ctx context.Context, q Querier, asOf time.Time) (rate float64, ok bool, err error) {
	var cohort, retained int64
	err = q.QueryRow(ctx, `
		WITH cohort AS (
		    SELECT id FROM users WHERE created_at::date = ($1::date - 7)
		)
		SELECT (SELECT count(*) FROM cohort),
		       (SELECT count(*)
		          FROM cohort c
		          JOIN user_activity_days a ON a.user_id = c.id AND a.day = $1::date)`,
		asOf).Scan(&cohort, &retained)
	if err != nil || cohort == 0 {
		return 0, false, err
	}
	return float64(retained) / float64(cohort), true, nil
}

// PlatformDistribution 统计近 days 天活跃设备的芯片架构分布。
func PlatformDistribution(ctx context.Context, q Querier, now time.Time, days int) ([]Bucket, error) {
	return bucketQuery(ctx, q, `
		SELECT CASE arch
		           WHEN 'arm64'  THEN 'macOS · Apple Silicon'
		           WHEN 'x86_64' THEN 'macOS · Intel'
		           ELSE '未知'
		       END AS label,
		       count(*)
		  FROM user_devices
		 WHERE last_seen_at >= $1::timestamptz - make_interval(days => $2::int)
		 GROUP BY label ORDER BY count(*) DESC`, now, days)
}

// AppVersionDistribution 统计近 days 天活跃设备的 APP 版本分布，用于评估强制升级时机。
func AppVersionDistribution(ctx context.Context, q Querier, now time.Time, days int) ([]Bucket, error) {
	return bucketQuery(ctx, q, `
		SELECT coalesce(nullif(app_version, ''), '未知') AS label, count(*)
		  FROM user_devices
		 WHERE last_seen_at >= $1::timestamptz - make_interval(days => $2::int)
		 GROUP BY label ORDER BY count(*) DESC`, now, days)
}

// SourceDistribution 统计近 days 天注册用户的来源分布，用于验证裂变效果。
func SourceDistribution(ctx context.Context, q Querier, now time.Time, days int) ([]Bucket, error) {
	return bucketQuery(ctx, q, `
		SELECT CASE registration_source
		           WHEN 'organic' THEN '自然流量'
		           WHEN 'invite'  THEN '好友邀请'
		           ELSE '其他渠道'
		       END AS label,
		       count(*)
		  FROM users
		 WHERE created_at >= $1::timestamptz - make_interval(days => $2::int)
		 GROUP BY label ORDER BY count(*) DESC`, now, days)
}

func bucketQuery(ctx context.Context, q Querier, sql string, args ...any) ([]Bucket, error) {
	rows, err := q.Query(ctx, sql, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	out := []Bucket{}
	for rows.Next() {
		var b Bucket
		if err := rows.Scan(&b.Label, &b.Count); err != nil {
			return nil, err
		}
		out = append(out, b)
	}
	return out, rows.Err()
}
