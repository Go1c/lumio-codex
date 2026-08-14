package store

import (
	"context"
	"encoding/json"
	"time"
)

// UpsertDevice 记录 APP 设备信息，支撑后台「使用平台」列与平台/版本分布。
func UpsertDevice(
	ctx context.Context, q Querier, userID int64,
	deviceID, platform, osVersion, arch, appVersion string, now time.Time,
) error {
	_, err := q.Exec(ctx, `
		INSERT INTO user_devices
		    (user_id, device_id, platform, os_version, arch, app_version, first_seen_at, last_seen_at)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $7)
		ON CONFLICT (user_id, device_id) DO UPDATE
		    SET platform = EXCLUDED.platform, os_version = EXCLUDED.os_version,
		        arch = EXCLUDED.arch, app_version = EXCLUDED.app_version,
		        last_seen_at = EXCLUDED.last_seen_at`,
		userID, deviceID, platform, osVersion, arch, appVersion, now)
	return err
}

// RecordActivity 记录用户当日活跃，重复调用无副作用。DAU 与留存都基于这张表。
func RecordActivity(ctx context.Context, q Querier, userID int64, day time.Time) error {
	_, err := q.Exec(ctx, `
		INSERT INTO user_activity_days (user_id, day) VALUES ($1, $2::date)
		ON CONFLICT DO NOTHING`, userID, day)
	return err
}

// LatestDeviceOf 返回用户最近使用的设备三元组，未登录过 APP 时三项均为空。
func LatestDeviceOf(ctx context.Context, q Querier, userID int64) (platform, osVersion, arch string, err error) {
	err = q.QueryRow(ctx, `
		SELECT platform, os_version, arch
		  FROM user_devices WHERE user_id = $1
		 ORDER BY last_seen_at DESC LIMIT 1`, userID).Scan(&platform, &osVersion, &arch)
	if err != nil {
		if normalizeErr(err) == ErrNotFound {
			return "", "", "", nil
		}
		return "", "", "", err
	}
	return platform, osVersion, arch, nil
}

// UserDevice 是用户的一台设备，供后台用户详情页展示。
type UserDevice struct {
	DeviceID    string
	Platform    string
	OSVersion   string
	Arch        string
	AppVersion  string
	FirstSeenAt time.Time
	LastSeenAt  time.Time
}

// ListUserDevices 列出用户的全部设备，最近使用的排在前面。
func ListUserDevices(ctx context.Context, q Querier, userID int64) ([]UserDevice, error) {
	rows, err := q.Query(ctx, `
		SELECT device_id, platform, os_version, arch, app_version, first_seen_at, last_seen_at
		  FROM user_devices
		 WHERE user_id = $1
		 ORDER BY last_seen_at DESC`, userID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var out []UserDevice
	for rows.Next() {
		var d UserDevice
		if err := rows.Scan(&d.DeviceID, &d.Platform, &d.OSVersion, &d.Arch,
			&d.AppVersion, &d.FirstSeenAt, &d.LastSeenAt); err != nil {
			return nil, err
		}
		out = append(out, d)
	}
	return out, rows.Err()
}

// —— 邮件发件箱 ——

// 邮件模板标识。
const (
	TemplateVerifyCode     = "verify_code"      // 注册验证码
	TemplateEmailChange    = "email_change"     // 改邮箱验证码（发往新邮箱）
	TemplateEmailChanged   = "email_changed"    // 改邮箱完成通知（发往原邮箱）
	TemplatePasswordReset  = "password_reset"   // 重设密码链接
	TemplateTrialGranted   = "trial_granted"    // 试用开通通知
	TemplateInviteRewarded = "invite_rewarded"  // 邀请奖励到账通知
	TemplateDeletionNotice = "deletion_pending" // 注销冷静期通知
)

// EnqueueEmail 把邮件放入发件箱。
//
// 业务事务只负责入队，实际投递由后台 worker 完成。这样注册链路不会被 SMTP 抖动拖垮，
// 测试也可以直接断言发件箱内容而不需要 SMTP 服务器。
func EnqueueEmail(ctx context.Context, q Querier, to, template string, payload map[string]any) error {
	encoded, err := json.Marshal(payload)
	if err != nil {
		return err
	}
	_, err = q.Exec(ctx,
		`INSERT INTO email_outbox (to_email, template, payload) VALUES ($1, $2, $3)`,
		to, template, encoded)
	return err
}

// OutboxMessage 是一封待投递邮件。
type OutboxMessage struct {
	ID       int64
	To       string
	Template string
	Payload  map[string]any
}

// ClaimPendingEmails 原子地领取一批待投递邮件：置为 sending 并刷新领取时间。
//
// 领取本身就是一条 UPDATE……RETURNING，提交后行锁立即释放——真正的 SMTP 投递
// 必须发生在事务之外，否则一次网络挂起就会连着行锁、连接池连接和整条 worker
// goroutine 一起拖死（QA S-8）。重试按 attempts 递增退避（每次 5 分钟、封顶
// 30 分钟），失败不再以 5 秒间隔连打。
func ClaimPendingEmails(ctx context.Context, q Querier, limit int) ([]OutboxMessage, error) {
	rows, err := q.Query(ctx, `
		WITH claimable AS (
		    SELECT id
		      FROM email_outbox
		     WHERE status = 'pending'
		       AND (last_attempt_at IS NULL
		            OR last_attempt_at < now() - (least(attempts, 6) * interval '5 minutes'))
		     ORDER BY created_at
		     LIMIT $1
		       FOR UPDATE SKIP LOCKED
		)
		UPDATE email_outbox e
		   SET status = 'sending', last_attempt_at = now(), attempts = attempts + 1
		  FROM claimable c
		 WHERE e.id = c.id
		RETURNING e.id, e.to_email, e.template, e.payload`, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var out []OutboxMessage
	for rows.Next() {
		var m OutboxMessage
		var raw []byte
		if err := rows.Scan(&m.ID, &m.To, &m.Template, &raw); err != nil {
			return nil, err
		}
		if err := json.Unmarshal(raw, &m.Payload); err != nil {
			return nil, err
		}
		out = append(out, m)
	}
	return out, rows.Err()
}

// MarkEmailSent 标记投递成功。attempts 已在领取时计入，这里只落终态。
func MarkEmailSent(ctx context.Context, q Querier, id int64, now time.Time) error {
	_, err := q.Exec(ctx,
		`UPDATE email_outbox SET status = 'sent', sent_at = $2, last_error = '' WHERE id = $1`,
		id, now)
	return err
}

// MarkEmailFailed 记录一次投递失败并退回 pending；累计失败 5 次后置为 failed 不再重试。
//
// attempts 已在领取时 +1，这里的 CASE 判断的是「本次已是第几次尝试」。
func MarkEmailFailed(ctx context.Context, q Querier, id int64, reason string) error {
	_, err := q.Exec(ctx, `
		UPDATE email_outbox
		   SET last_error = $2,
		       status = CASE WHEN attempts >= 5 THEN 'failed' ELSE 'pending' END
		 WHERE id = $1`, id, reason)
	return err
}

// RequeueStaleSendingEmails 把停滞在 sending 的行收回 pending。
//
// worker 在「领取已提交、结果未回写」之间崩溃时会留下 sending 行；不回收它们
// 就永远不会再被领取（QA S-8）。由维护任务周期调用。
func RequeueStaleSendingEmails(ctx context.Context, q Querier, staleBefore time.Time) (int64, error) {
	tag, err := q.Exec(ctx, `
		UPDATE email_outbox
		   SET status = 'pending'
		 WHERE status = 'sending' AND last_attempt_at < $1`, staleBefore)
	if err != nil {
		return 0, err
	}
	return tag.RowsAffected(), nil
}

// —— 审计日志 ——

// AuditEntry 是一条审计记录。
type AuditEntry struct {
	ActorType  string
	ActorID    string
	Action     string
	TargetType string
	TargetID   string
	Before     any
	After      any
	IP         string
	UserAgent  string
}

// WriteAudit 记录一次操作，含操作人、时间与前后值（交互设计 7.5）。
func WriteAudit(ctx context.Context, q Querier, e AuditEntry) error {
	before, err := marshalNullable(e.Before)
	if err != nil {
		return err
	}
	after, err := marshalNullable(e.After)
	if err != nil {
		return err
	}

	_, err = q.Exec(ctx, `
		INSERT INTO audit_logs
		    (actor_type, actor_id, action, target_type, target_id, before, after, ip, user_agent)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)`,
		e.ActorType, e.ActorID, e.Action, e.TargetType, e.TargetID, before, after, e.IP, e.UserAgent)
	return err
}

func marshalNullable(v any) ([]byte, error) {
	if v == nil {
		return nil, nil
	}
	return json.Marshal(v)
}

// AuditRecord 是回读的审计记录。
type AuditRecord struct {
	ID         int64           `json:"id"`
	ActorType  string          `json:"actor_type"`
	ActorID    string          `json:"actor_id"`
	Action     string          `json:"action"`
	TargetType string          `json:"target_type"`
	TargetID   string          `json:"target_id"`
	Before     json.RawMessage `json:"before,omitempty"`
	After      json.RawMessage `json:"after,omitempty"`
	IP         string          `json:"ip"`
	CreatedAt  time.Time       `json:"created_at"`
}

// ListAuditLogs 分页读取审计日志；actor 匹配 actor_id，action 精确匹配，空串表示不筛选。
//
// 审计日志同时承载禁用用户、退款、配置改动与查看用户详情，量一大就必须能按人、按动作收敛。
// M1 的审计量在千行量级，ix_audit_logs_created 已覆盖默认的时间倒序分页，故未为 action 单独建索引。
func ListAuditLogs(
	ctx context.Context, q Querier, actor, action string, limit, offset int,
) ([]AuditRecord, int64, error) {
	// 空串表示不筛选。显式 ::text 转换是必需的：$1 = '' 两侧都是未定类型时，
	// PostgreSQL 会拒绝推断参数类型。
	const filter = `
		 WHERE ($1::text = '' OR actor_id = $1)
		   AND ($2::text = '' OR action = $2)`

	var total int64
	if err := q.QueryRow(ctx,
		`SELECT count(*) FROM audit_logs`+filter, actor, action).Scan(&total); err != nil {
		return nil, 0, err
	}

	rows, err := q.Query(ctx, `
		SELECT id, actor_type, actor_id, action, target_type, target_id, before, after, ip, created_at
		  FROM audit_logs`+filter+`
		 ORDER BY created_at DESC LIMIT $3 OFFSET $4`, actor, action, limit, offset)
	if err != nil {
		return nil, 0, err
	}
	defer rows.Close()

	var out []AuditRecord
	for rows.Next() {
		var r AuditRecord
		if err := rows.Scan(&r.ID, &r.ActorType, &r.ActorID, &r.Action, &r.TargetType,
			&r.TargetID, &r.Before, &r.After, &r.IP, &r.CreatedAt); err != nil {
			return nil, 0, err
		}
		out = append(out, r)
	}
	return out, total, rows.Err()
}
