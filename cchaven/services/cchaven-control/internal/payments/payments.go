// Package payments 抽象支付渠道。
//
// M1 只实现 mock adapter 打通下单→回调→入账链路；支付宝与微信支付按同一接口接入，
// 上层的订单、订阅入账与退款逻辑无需改动。
package payments

import (
	"context"
	"errors"
	"fmt"
	"sync"
	"time"
)

// ErrNotImplemented 表示该渠道尚未接入。
var ErrNotImplemented = errors.New("payments: 该支付渠道尚未接入")

// ErrInvalidSignature 表示回调验签失败。
var ErrInvalidSignature = errors.New("payments: 回调验签失败")

// CreateRequest 是发起支付的请求。
type CreateRequest struct {
	OrderNo     string
	AmountCents int64
	Currency    string
	Subject     string
	NotifyURL   string
	ReturnURL   string
}

// CreateResponse 是发起支付的结果。PayURL 指向支付服务商的托管页面，
// 站内绝不收集卡号（交互设计 5.6）。
type CreateResponse struct {
	PayURL    string
	ExpiresAt time.Time
}

// Notification 是解析并验签后的支付回调。
type Notification struct {
	OrderNo string
	TxnID   string
	Paid    bool
	Amount  int64
}

// RefundRequest 是退款请求。
type RefundRequest struct {
	OrderNo     string
	RefundID    string
	AmountCents int64
	Reason      string
}

// RefundResponse 是退款结果。异步渠道返回 Pending，等待回调终结。
type RefundResponse struct {
	ProviderRefundID string
	Succeeded        bool
}

// Provider 是支付渠道适配器。
type Provider interface {
	// Name 返回渠道标识，与 orders.channel 取值一致。
	Name() string
	// CreatePayment 创建支付单并返回托管收银台地址。
	CreatePayment(ctx context.Context, req CreateRequest) (CreateResponse, error)
	// ParseNotification 校验签名并解析回调内容。
	ParseNotification(payload []byte, signature string) (Notification, error)
	// Refund 发起退款。
	Refund(ctx context.Context, req RefundRequest) (RefundResponse, error)
}

// Registry 按渠道名索引已接入的适配器。
type Registry struct {
	mu        sync.RWMutex
	providers map[string]Provider
}

// NewRegistry 构造空的适配器注册表。
func NewRegistry() *Registry {
	return &Registry{providers: map[string]Provider{}}
}

// Register 注册一个渠道适配器。
func (r *Registry) Register(p Provider) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.providers[p.Name()] = p
}

// Get 取出渠道适配器；未接入的渠道返回 ErrNotImplemented。
func (r *Registry) Get(name string) (Provider, error) {
	r.mu.RLock()
	defer r.mu.RUnlock()

	p, ok := r.providers[name]
	if !ok {
		return nil, fmt.Errorf("%w: %s", ErrNotImplemented, name)
	}
	return p, nil
}

// Channels 返回已接入的渠道名列表，供前端展示可用支付方式。
func (r *Registry) Channels() []string {
	r.mu.RLock()
	defer r.mu.RUnlock()

	out := make([]string, 0, len(r.providers))
	for name := range r.providers {
		out = append(out, name)
	}
	return out
}
