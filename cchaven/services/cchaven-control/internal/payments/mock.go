package payments

import (
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"time"
)

// Mock 是开发与测试用的支付渠道。
//
// 它把「托管收银台」简化为一个本地页面地址，并用 HMAC 模拟回调验签，
// 使得下单 → 回调 → 订阅入账的完整链路可以在没有真实支付账号的情况下跑通。
type Mock struct {
	baseURL string
	secret  []byte
}

// NewMock 构造 mock 渠道。
func NewMock(baseURL string, secret []byte) *Mock {
	return &Mock{baseURL: baseURL, secret: secret}
}

// Name 实现 Provider。
func (m *Mock) Name() string { return "mock" }

// CreatePayment 返回一个本地模拟收银台地址。
func (m *Mock) CreatePayment(_ context.Context, req CreateRequest) (CreateResponse, error) {
	return CreateResponse{
		PayURL:    fmt.Sprintf("%s/mock-checkout?order_no=%s", m.baseURL, req.OrderNo),
		ExpiresAt: time.Now().Add(30 * time.Minute),
	}, nil
}

// MockNotification 是 mock 渠道的回调报文。
type MockNotification struct {
	OrderNo string `json:"order_no"`
	TxnID   string `json:"txn_id"`
	Paid    bool   `json:"paid"`
	Amount  int64  `json:"amount_cents"`
}

// ParseNotification 用 HMAC-SHA256 校验回调签名并解析报文。
func (m *Mock) ParseNotification(payload []byte, signature string) (Notification, error) {
	if !hmac.Equal([]byte(m.Sign(payload)), []byte(signature)) {
		return Notification{}, ErrInvalidSignature
	}

	var body MockNotification
	if err := json.Unmarshal(payload, &body); err != nil {
		return Notification{}, fmt.Errorf("payments: 回调报文解析失败: %w", err)
	}

	return Notification{
		OrderNo: body.OrderNo,
		TxnID:   body.TxnID,
		Paid:    body.Paid,
		Amount:  body.Amount,
	}, nil
}

// Sign 生成回调签名，供测试与本地模拟收银台构造合法回调。
func (m *Mock) Sign(payload []byte) string {
	mac := hmac.New(sha256.New, m.secret)
	mac.Write(payload)
	return hex.EncodeToString(mac.Sum(nil))
}

// Refund 立即返回成功，模拟同步退款渠道。
func (m *Mock) Refund(_ context.Context, req RefundRequest) (RefundResponse, error) {
	return RefundResponse{ProviderRefundID: "mock-refund-" + req.RefundID, Succeeded: true}, nil
}
