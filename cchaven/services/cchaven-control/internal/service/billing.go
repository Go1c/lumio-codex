package service

import (
	"context"
	"errors"
	"log/slog"
	"time"

	"github.com/jackc/pgx/v5"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/apperr"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/db"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/domain"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/payments"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/store"
)

// Plan 是唯一的包月套餐，价格从运营配置读取，页面不写死。
type Plan struct {
	Name        string   `json:"name"`
	AmountCents int64    `json:"amount_cents"`
	Currency    string   `json:"currency"`
	PeriodUnit  string   `json:"period_unit"`
	Channels    []string `json:"channels"`
}

// Plan 返回当前包月套餐。
func (s *Service) Plan(ctx context.Context) (Plan, error) {
	cfg, err := store.LoadOpsConfig(ctx, s.Pool)
	if err != nil {
		return Plan{}, err
	}
	return Plan{
		Name:        "CC避风港包月",
		AmountCents: cfg.PricingMonthly.AmountCents,
		Currency:    cfg.PricingMonthly.Currency,
		PeriodUnit:  "month",
		Channels:    s.Payments.Channels(),
	}, nil
}

// CheckoutResult 是下单结果。
type CheckoutResult struct {
	OrderNo     string    `json:"order_no"`
	PayURL      string    `json:"pay_url"`
	AmountCents int64     `json:"amount_cents"`
	Currency    string    `json:"currency"`
	ExpiresAt   time.Time `json:"expires_at"`
}

// Checkout 创建订单并向支付渠道申请托管收银台地址。付款只在官网完成。
func (s *Service) Checkout(
	ctx context.Context, userID int64, channel string, idempotencyKey string,
) (CheckoutResult, error) {
	provider, err := s.Payments.Get(channel)
	if err != nil {
		return CheckoutResult{}, apperr.InvalidParams().WithCause(err)
	}

	cfg, err := store.LoadOpsConfig(ctx, s.Pool)
	if err != nil {
		return CheckoutResult{}, err
	}

	now := s.now()
	var order domain.Order

	err = db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		orderNo, err := store.NextOrderNo(ctx, tx, now)
		if err != nil {
			return err
		}
		order, err = store.CreateOrder(ctx, tx, store.CreateOrderParams{
			OrderNo:        orderNo,
			UserID:         userID,
			AmountCents:    cfg.PricingMonthly.AmountCents,
			Currency:       cfg.PricingMonthly.Currency,
			Channel:        domain.PaymentChannel(channel),
			PeriodMonths:   1,
			Provider:       provider.Name(),
			IdempotencyKey: idempotencyKey,
		})
		return err
	})
	if err != nil {
		return CheckoutResult{}, err
	}

	payment, err := provider.CreatePayment(ctx, payments.CreateRequest{
		OrderNo:     order.OrderNo,
		AmountCents: order.AmountCents,
		Currency:    order.Currency,
		Subject:     "CC避风港包月",
		NotifyURL:   s.Cfg.PublicURL + "/api/v1/billing/webhook/" + provider.Name(),
		ReturnURL:   s.Cfg.PublicURL + "/account",
	})
	if err != nil {
		return CheckoutResult{}, err
	}

	return CheckoutResult{
		OrderNo:     order.OrderNo,
		PayURL:      payment.PayURL,
		AmountCents: order.AmountCents,
		Currency:    order.Currency,
		ExpiresAt:   payment.ExpiresAt,
	}, nil
}

// HandleWebhook 处理支付回调：验签 → 幂等入账 → 延长订阅。
//
// 回调可能被重复投递，因此入账走 subscription_events 的唯一索引兜底，
// 同一订单号只会延长一次订阅。
func (s *Service) HandleWebhook(ctx context.Context, channel string, payload []byte, signature string) error {
	provider, err := s.Payments.Get(channel)
	if err != nil {
		return apperr.NotFound().WithCause(err)
	}

	notification, err := provider.ParseNotification(payload, signature)
	if err != nil {
		// 验签失败也要留痕，便于排查伪造回调与配置错误。
		_ = store.RecordPaymentEvent(ctx, s.Pool, nil, "notify", channel, string(payload), false)
		if errors.Is(err, payments.ErrInvalidSignature) {
			return apperr.Forbidden().WithCause(err)
		}
		return apperr.InvalidParams().WithCause(err)
	}

	now := s.now()
	var reject error

	err = db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		order, err := store.LockOrderByNo(ctx, tx, notification.OrderNo)
		if err != nil {
			if errors.Is(err, store.ErrNotFound) {
				return apperr.NotFound()
			}
			return err
		}

		if !notification.Paid {
			if err := store.RecordPaymentEvent(
				ctx, tx, &order.ID, "notify", channel, string(payload), true,
			); err != nil {
				return err
			}
			if order.Status == domain.OrderPending {
				return store.UpdateOrderStatus(ctx, tx, order.OrderNo, domain.OrderFailed, nil, nil, now)
			}
			return nil
		}
		if order.Status != domain.OrderPending {
			// 重复回调也要留痕后确认。
			return store.RecordPaymentEvent(
				ctx, tx, &order.ID, "notify", channel, string(payload), true,
			)
		}

		// 金额必须与下单一致（QA S-5）：签名合法不代表金额没被动过——
		// 渠道侧改价、回调金额被篡改，都会把「1 分钱入账整单」的订单刷成已支付。
		// 留痕（ok=false）后拒绝，订单停在 pending 等待真实回调或人工核对；
		// 事件必须提交落盘，错误改在事务外返回，否则回滚会连留痕一起吞掉。
		if notification.Amount != order.AmountCents {
			slog.Error("支付回调金额与订单不符，拒绝入账",
				"order_no", order.OrderNo, "channel", channel,
				"order_amount_cents", order.AmountCents,
				"notified_amount_cents", notification.Amount)
			if err := store.RecordPaymentEvent(
				ctx, tx, &order.ID, "notify", channel, string(payload), false,
			); err != nil {
				return err
			}
			reject = apperr.PaymentAmountMismatch()
			return nil
		}

		if err := store.RecordPaymentEvent(
			ctx, tx, &order.ID, "notify", channel, string(payload), true,
		); err != nil {
			return err
		}

		txnID := notification.TxnID
		if err := store.UpdateOrderStatus(
			ctx, tx, order.OrderNo, domain.OrderPaid, &txnID, &now, now,
		); err != nil {
			return err
		}

		_, err = s.CreditPurchase(ctx, tx, order.UserID, order.OrderNo, order.PeriodMonths)
		return err
	})
	if reject != nil {
		return reject
	}
	return err
}

// OrderView 是订单的对外表示。
type OrderView struct {
	OrderNo     string     `json:"order_no"`
	AmountCents int64      `json:"amount_cents"`
	Currency    string     `json:"currency"`
	Channel     string     `json:"channel"`
	Status      string     `json:"status"`
	PaidAt      *time.Time `json:"paid_at,omitempty"`
	CreatedAt   time.Time  `json:"created_at"`
}

// ListMyOrders 列出当前用户的订单。
func (s *Service) ListMyOrders(ctx context.Context, userID int64) ([]OrderView, error) {
	orders, err := store.ListUserOrders(ctx, s.Pool, userID, 50)
	if err != nil {
		return nil, err
	}

	out := make([]OrderView, 0, len(orders))
	for _, o := range orders {
		out = append(out, viewOrder(o))
	}
	return out, nil
}

// GetMyOrder 查询单笔订单，供支付完成后轮询状态。
func (s *Service) GetMyOrder(ctx context.Context, userID int64, orderNo string) (OrderView, error) {
	order, err := store.GetOrderByNo(ctx, s.Pool, orderNo)
	if err != nil {
		if errors.Is(err, store.ErrNotFound) {
			return OrderView{}, apperr.NotFound()
		}
		return OrderView{}, err
	}
	if order.UserID != userID {
		return OrderView{}, apperr.NotFound()
	}
	return viewOrder(order), nil
}

func viewOrder(o domain.Order) OrderView {
	return OrderView{
		OrderNo:     o.OrderNo,
		AmountCents: o.AmountCents,
		Currency:    o.Currency,
		Channel:     string(o.Channel),
		Status:      string(o.Status),
		PaidAt:      o.PaidAt,
		CreatedAt:   o.CreatedAt,
	}
}
