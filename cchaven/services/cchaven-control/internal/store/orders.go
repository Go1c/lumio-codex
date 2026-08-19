package store

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/domain"
)

// NextOrderNo 生成当日订单号，格式 CC{YYYYMMDD}-{6 位序号}（原型示例 CC20260812-100486）。
// 用独立序号表 + UPSERT RETURNING 取号，并发下不会重号也不会跳号。
func NextOrderNo(ctx context.Context, q Querier, now time.Time) (string, error) {
	// 序号键与订单号前缀取自同一个 UTC 日期，避免跨零点时两者落到不同的天。
	day := now.UTC()

	var seq int64
	err := q.QueryRow(ctx, `
		INSERT INTO order_sequences (day, next_seq)
		VALUES ($1::date, 100001)
		ON CONFLICT (day) DO UPDATE SET next_seq = order_sequences.next_seq + 1
		RETURNING next_seq`, day.Format("2006-01-02")).Scan(&seq)
	if err != nil {
		return "", err
	}
	return fmt.Sprintf("CC%s-%06d", day.Format("20060102"), seq), nil
}

// CreateOrderParams 描述一次下单。
type CreateOrderParams struct {
	OrderNo        string
	UserID         int64
	AmountCents    int64
	Currency       string
	Channel        domain.PaymentChannel
	PeriodMonths   int
	Provider       string
	IdempotencyKey string
}

// CreateOrder 建立待支付订单。
func CreateOrder(ctx context.Context, q Querier, p CreateOrderParams) (domain.Order, error) {
	var o domain.Order
	err := q.QueryRow(ctx, `
		INSERT INTO orders (order_no, user_id, amount_cents, currency, channel,
		                    period_months, provider, idempotency_key)
		VALUES ($1, $2, $3, $4, $5, $6, $7, nullif($8, ''))
		RETURNING id, order_no, user_id, amount_cents, currency, channel, status,
		          period_months, provider, provider_txn_id, paid_at, created_at`,
		p.OrderNo, p.UserID, p.AmountCents, p.Currency, p.Channel,
		p.PeriodMonths, p.Provider, p.IdempotencyKey).Scan(
		&o.ID, &o.OrderNo, &o.UserID, &o.AmountCents, &o.Currency, &o.Channel, &o.Status,
		&o.PeriodMonths, &o.Provider, &o.ProviderTxnID, &o.PaidAt, &o.CreatedAt)
	if err != nil {
		return domain.Order{}, normalizeErr(err)
	}
	return o, nil
}

const orderColumns = `
	o.id, o.order_no, o.user_id, u.email, o.amount_cents, o.currency, o.channel, o.status,
	o.period_months, o.provider, o.provider_txn_id, o.paid_at, o.created_at`

func scanOrder(row interface{ Scan(...any) error }) (domain.Order, error) {
	var o domain.Order
	err := row.Scan(
		&o.ID, &o.OrderNo, &o.UserID, &o.UserEmail, &o.AmountCents, &o.Currency, &o.Channel,
		&o.Status, &o.PeriodMonths, &o.Provider, &o.ProviderTxnID, &o.PaidAt, &o.CreatedAt)
	if err != nil {
		return domain.Order{}, normalizeErr(err)
	}
	return o, nil
}

// GetOrderByNo 按订单号读取。
func GetOrderByNo(ctx context.Context, q Querier, orderNo string) (domain.Order, error) {
	return scanOrder(q.QueryRow(ctx,
		`SELECT `+orderColumns+` FROM orders o JOIN users u ON u.id = o.user_id WHERE o.order_no = $1`,
		orderNo))
}

// GetOrderByIdempotencyKey 按幂等键读取；空键视为未命中。
func GetOrderByIdempotencyKey(ctx context.Context, q Querier, key string) (domain.Order, error) {
	key = strings.TrimSpace(key)
	if key == "" {
		return domain.Order{}, ErrNotFound
	}
	return scanOrder(q.QueryRow(ctx,
		`SELECT `+orderColumns+` FROM orders o JOIN users u ON u.id = o.user_id WHERE o.idempotency_key = $1`,
		key))
}

// LockOrderByNo 取行级锁后读取，用于支付回调与退款的串行处理。
func LockOrderByNo(ctx context.Context, q Querier, orderNo string) (domain.Order, error) {
	return scanOrder(q.QueryRow(ctx,
		`SELECT `+orderColumns+` FROM orders o JOIN users u ON u.id = o.user_id
		  WHERE o.order_no = $1 FOR UPDATE OF o`, orderNo))
}

// ListUserOrders 列出用户自己的订单。
func ListUserOrders(ctx context.Context, q Querier, userID int64, limit int) ([]domain.Order, error) {
	rows, err := q.Query(ctx,
		`SELECT `+orderColumns+` FROM orders o JOIN users u ON u.id = o.user_id
		  WHERE o.user_id = $1 ORDER BY o.created_at DESC LIMIT $2`, userID, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	return collectOrders(rows)
}

// ListOrders 供后台按状态分页查询；status 为空表示全部。
func ListOrders(
	ctx context.Context, q Querier, status string, limit, offset int,
) ([]domain.Order, int64, error) {
	// 空串表示不筛选。显式 ::text 转换是必需的：$1 = '' 两侧都是未定类型时，
	// PostgreSQL 会拒绝推断参数类型。
	var total int64
	if err := q.QueryRow(ctx,
		`SELECT count(*) FROM orders WHERE ($1::text = '' OR status = $1)`, status).Scan(&total); err != nil {
		return nil, 0, err
	}

	rows, err := q.Query(ctx,
		`SELECT `+orderColumns+` FROM orders o JOIN users u ON u.id = o.user_id
		  WHERE ($1::text = '' OR o.status = $1)
		  ORDER BY o.created_at DESC LIMIT $2 OFFSET $3`, status, limit, offset)
	if err != nil {
		return nil, 0, err
	}
	defer rows.Close()

	orders, err := collectOrders(rows)
	return orders, total, err
}

func collectOrders(rows interface {
	Next() bool
	Scan(...any) error
	Err() error
},
) ([]domain.Order, error) {
	var out []domain.Order
	for rows.Next() {
		o, err := scanOrder(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, o)
	}
	return out, rows.Err()
}

// UpdateOrderStatus 迁移订单状态。
func UpdateOrderStatus(
	ctx context.Context, q Querier, orderNo string, status domain.OrderStatus,
	txnID *string, paidAt *time.Time, now time.Time,
) error {
	_, err := q.Exec(ctx, `
		UPDATE orders
		   SET status = $2,
		       provider_txn_id = coalesce($3, provider_txn_id),
		       paid_at = coalesce($4, paid_at),
		       updated_at = $5
		 WHERE order_no = $1`, orderNo, status, txnID, paidAt, now)
	return err
}

// TodayOrderSummary 统计当日已支付订单笔数与金额，供后台页头与仪表盘使用。
func TodayOrderSummary(ctx context.Context, q Querier, dayStart, dayEnd time.Time) (int64, int64, error) {
	var count, amount int64
	err := q.QueryRow(ctx, `
		SELECT count(*), coalesce(sum(amount_cents), 0)
		  FROM orders
		 WHERE status = 'paid' AND paid_at >= $1 AND paid_at < $2`,
		dayStart, dayEnd).Scan(&count, &amount)
	return count, amount, err
}

// RecordPaymentEvent 落库支付回调原文，便于对账与排查。
func RecordPaymentEvent(
	ctx context.Context, q Querier, orderID *int64, eventType, provider string,
	payload any, signatureOK bool,
) error {
	encoded, err := json.Marshal(payload)
	if err != nil {
		return err
	}
	_, err = q.Exec(ctx, `
		INSERT INTO payment_events (order_id, type, provider, payload, signature_ok)
		VALUES ($1, $2, $3, $4, $5)`, orderID, eventType, provider, encoded, signatureOK)
	return err
}

// CreateRefund 建立退款单。
func CreateRefund(
	ctx context.Context, q Querier, orderID, adminID int64, amountCents int64, reason string,
) (int64, error) {
	var id int64
	err := q.QueryRow(ctx, `
		INSERT INTO refunds (order_id, amount_cents, requested_by_admin_id, reason)
		VALUES ($1, $2, $3, $4) RETURNING id`, orderID, amountCents, adminID, reason).Scan(&id)
	return id, err
}

// CompleteRefund 结束退款流程。
func CompleteRefund(
	ctx context.Context, q Querier, id int64, status, providerRefundID string, now time.Time,
) error {
	_, err := q.Exec(ctx, `
		UPDATE refunds
		   SET status = $2, provider_refund_id = nullif($3, ''), completed_at = $4
		 WHERE id = $1`, id, status, providerRefundID, now)
	return err
}
