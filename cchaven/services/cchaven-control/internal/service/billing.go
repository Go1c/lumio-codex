package service

import (
	"context"
	"errors"
	"log/slog"
	"math"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/apperr"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/db"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/domain"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/payments"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/store"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/sub2api"
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

const debitPurpose = "cchaven_monthly"

// PayWithBalanceResult 是余额支付开通包月的结果。
type PayWithBalanceResult struct {
	Order       OrderView          `json:"order"`
	Entitlement domain.Entitlement `json:"entitlement"`
}

// PayWithBalance 用 Sub2API 余额扣当月套餐并入账。
//
// 已是 active 的用户可以再买一个月（顺延 30 天）。扣费在建单事务外进行：
// 余额不足把订单标 failed；上游不可用则保持 pending，绝不入账。
func (s *Service) PayWithBalance(
	ctx context.Context, userID int64, userToken string, idempotencyKey string,
) (PayWithBalanceResult, error) {
	if s.Sub2API == nil {
		return PayWithBalanceResult{}, apperr.IdentityUnavailable()
	}

	identity, err := s.Sub2API.VerifyFresh(ctx, userToken)
	switch {
	case errors.Is(err, sub2api.ErrInvalidToken):
		return PayWithBalanceResult{}, apperr.Unauthorized()
	case errors.Is(err, sub2api.ErrUnavailable):
		return PayWithBalanceResult{}, apperr.IdentityUnavailable().WithCause(err)
	case err != nil:
		return PayWithBalanceResult{}, err
	}

	ops, err := store.LoadOpsConfig(ctx, s.Pool)
	if err != nil {
		return PayWithBalanceResult{}, err
	}
	amountCents := ops.PricingMonthly.AmountCents
	currency := ops.PricingMonthly.Currency
	if currency == "" {
		currency = "CNY"
	}

	idempotencyKey = strings.TrimSpace(idempotencyKey)
	if idempotencyKey != "" {
		existing, getErr := store.GetOrderByIdempotencyKey(ctx, s.Pool, idempotencyKey)
		if getErr == nil && existing.UserID == userID && existing.Status == domain.OrderPaid {
			// 已付清的重放必须在余额门槛之前返回：第一次扣费后余额可能已不够再买一个月。
			return s.payWithBalanceResult(ctx, userID, existing)
		}
		if getErr != nil && !errors.Is(getErr, store.ErrNotFound) {
			return PayWithBalanceResult{}, getErr
		}
	}

	if int64(math.Round(identity.Balance*100)) < amountCents {
		return PayWithBalanceResult{}, apperr.InsufficientBalance(s.Cfg.PurchaseURL())
	}

	now := s.now()
	var order domain.Order

	err = db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		created, createErr := s.orderForBalancePay(ctx, tx, userID, amountCents, currency, idempotencyKey, now)
		if createErr != nil {
			return createErr
		}
		order = created
		return nil
	})
	if err != nil {
		if existing, ok := s.orderAfterIdempotencyConflict(ctx, userID, idempotencyKey, err); ok {
			order = existing
		} else {
			return PayWithBalanceResult{}, err
		}
	}

	if order.Status == domain.OrderPaid {
		return s.payWithBalanceResult(ctx, userID, order)
	}

	debit, err := s.Sub2API.Debit(ctx, userToken, order.OrderNo, sub2api.DebitRequest{
		Amount:   float64(order.AmountCents) / 100,
		Currency: order.Currency,
		Purpose:  debitPurpose,
		Ref:      order.OrderNo,
	})
	switch {
	case errors.Is(err, sub2api.ErrInsufficientBalance):
		if markErr := store.UpdateOrderStatus(
			ctx, s.Pool, order.OrderNo, domain.OrderFailed, nil, nil, s.now(),
		); markErr != nil {
			return PayWithBalanceResult{}, markErr
		}
		return PayWithBalanceResult{}, apperr.InsufficientBalance(s.Cfg.PurchaseURL())
	case errors.Is(err, sub2api.ErrInvalidToken):
		return PayWithBalanceResult{}, apperr.Unauthorized()
	case err != nil:
		return PayWithBalanceResult{}, apperr.DebitUnavailable().WithCause(err)
	}

	err = db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		locked, lockErr := store.LockOrderByNo(ctx, tx, order.OrderNo)
		if lockErr != nil {
			return lockErr
		}
		if locked.Status == domain.OrderPaid {
			order = locked
			return nil
		}

		paidAt := s.now()
		txnID := debit.TxnID
		if updErr := store.UpdateOrderStatus(
			ctx, tx, locked.OrderNo, domain.OrderPaid, &txnID, &paidAt, paidAt,
		); updErr != nil {
			return updErr
		}
		if _, creditErr := s.CreditPurchase(ctx, tx, locked.UserID, locked.OrderNo, locked.PeriodMonths); creditErr != nil {
			return creditErr
		}
		paid, getErr := store.GetOrderByNo(ctx, tx, locked.OrderNo)
		if getErr != nil {
			return getErr
		}
		order = paid
		return nil
	})
	if err != nil {
		return PayWithBalanceResult{}, err
	}
	return s.payWithBalanceResult(ctx, userID, order)
}

func (s *Service) orderForBalancePay(
	ctx context.Context, tx pgx.Tx, userID, amountCents int64, currency, idempotencyKey string, now time.Time,
) (domain.Order, error) {
	if idempotencyKey != "" {
		existing, err := store.GetOrderByIdempotencyKey(ctx, tx, idempotencyKey)
		if err == nil {
			if existing.UserID != userID {
				return domain.Order{}, apperr.InvalidParams()
			}
			return existing, nil
		}
		if !errors.Is(err, store.ErrNotFound) {
			return domain.Order{}, err
		}
	}

	orderNo, err := store.NextOrderNo(ctx, tx, now)
	if err != nil {
		return domain.Order{}, err
	}
	return store.CreateOrder(ctx, tx, store.CreateOrderParams{
		OrderNo:        orderNo,
		UserID:         userID,
		AmountCents:    amountCents,
		Currency:       currency,
		Channel:        domain.ChannelBalance,
		PeriodMonths:   1,
		Provider:       "sub2api",
		IdempotencyKey: idempotencyKey,
	})
}

func (s *Service) orderAfterIdempotencyConflict(
	ctx context.Context, userID int64, idempotencyKey string, err error,
) (domain.Order, bool) {
	if !store.IsUniqueViolation(err) || idempotencyKey == "" {
		return domain.Order{}, false
	}
	existing, getErr := store.GetOrderByIdempotencyKey(ctx, s.Pool, idempotencyKey)
	if getErr != nil || existing.UserID != userID {
		return domain.Order{}, false
	}
	return existing, true
}

func (s *Service) payWithBalanceResult(
	ctx context.Context, userID int64, order domain.Order,
) (PayWithBalanceResult, error) {
	entitlement, err := s.Entitlement(ctx, userID)
	if err != nil {
		return PayWithBalanceResult{}, err
	}
	return PayWithBalanceResult{Order: viewOrder(order), Entitlement: entitlement}, nil
}
