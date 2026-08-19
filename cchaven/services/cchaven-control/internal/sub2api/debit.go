package sub2api

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strings"
)

const (
	// DefaultDebitPath 是用账户余额扣一笔费用的端点。
	DefaultDebitPath = "/api/v1/user/balance/debit"
)

var (
	// ErrInsufficientBalance 表示账户余额不够支付这一笔。
	ErrInsufficientBalance = errors.New("sub2api: 余额不足")
	// ErrDebitUnavailable 表示扣费接口不可用，调用方应返回 503 而不是入账。
	ErrDebitUnavailable = errors.New("sub2api: 扣费不可用")
)

// DebitRequest 是一笔余额扣费。金额单位是元。
type DebitRequest struct {
	Amount   float64 `json:"amount"`
	Currency string  `json:"currency"`
	Purpose  string  `json:"purpose"`
	Ref      string  `json:"ref"`
}

// DebitResult 是上游确认入账后的回执。
type DebitResult struct {
	TxnID    string  `json:"txn_id"`
	Amount   float64 `json:"amount"`
	Balance  float64 `json:"balance"`
	Currency string  `json:"currency"`
}

// Debit 用用户令牌从 Sub2API 余额里扣一笔。
//
// 幂等由上游按 Idempotency-Key 保证：同一把钥匙重放必须回同一 txn_id，
// 本客户端每次都会发出请求，不在本地吞第二次。
func (c *Client) Debit(ctx context.Context, userToken string, idempotencyKey string, req DebitRequest) (DebitResult, error) {
	userToken = strings.TrimSpace(userToken)
	if userToken == "" {
		return DebitResult{}, ErrInvalidToken
	}

	payload, err := json.Marshal(req)
	if err != nil {
		return DebitResult{}, fmt.Errorf("%w: 编码请求失败: %v", ErrDebitUnavailable, err)
	}

	httpReq, err := http.NewRequestWithContext(ctx, http.MethodPost, c.baseURL+DefaultDebitPath, bytes.NewReader(payload))
	if err != nil {
		return DebitResult{}, fmt.Errorf("%w: 构造请求失败: %v", ErrDebitUnavailable, err)
	}
	httpReq.Header.Set("Authorization", "Bearer "+userToken)
	httpReq.Header.Set("Accept", "application/json")
	httpReq.Header.Set("Content-Type", "application/json")
	httpReq.Header.Set("Idempotency-Key", strings.TrimSpace(idempotencyKey))

	resp, err := c.http.Do(httpReq)
	if err != nil {
		return DebitResult{}, fmt.Errorf("%w: %v", ErrDebitUnavailable, err)
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(io.LimitReader(resp.Body, maxBodyBytes))
	if err != nil {
		return DebitResult{}, fmt.Errorf("%w: 读取响应失败: %v", ErrDebitUnavailable, err)
	}
	return parseDebit(resp.StatusCode, body)
}

func parseDebit(status int, raw []byte) (DebitResult, error) {
	var envelope struct {
		Code   int             `json:"code"`
		Reason string          `json:"reason"`
		Data   json.RawMessage `json:"data"`
	}
	if err := json.Unmarshal(raw, &envelope); err != nil && status == http.StatusOK {
		return DebitResult{}, fmt.Errorf("%w: 响应无法解析: %v", ErrDebitUnavailable, err)
	}
	if strings.EqualFold(strings.TrimSpace(envelope.Reason), "INSUFFICIENT_BALANCE") {
		return DebitResult{}, ErrInsufficientBalance
	}
	if status == http.StatusUnauthorized {
		return DebitResult{}, ErrInvalidToken
	}
	if status != http.StatusOK || envelope.Code != 0 {
		return DebitResult{}, fmt.Errorf("%w: 上游返回 HTTP %d", ErrDebitUnavailable, status)
	}

	var data struct {
		TxnID    flexString `json:"txn_id"`
		Amount   flexFloat  `json:"amount"`
		Balance  flexFloat  `json:"balance"`
		Currency string     `json:"currency"`
	}
	if err := json.Unmarshal(envelope.Data, &data); err != nil {
		return DebitResult{}, fmt.Errorf("%w: 响应无法解析: %v", ErrDebitUnavailable, err)
	}
	if strings.TrimSpace(string(data.TxnID)) == "" {
		return DebitResult{}, fmt.Errorf("%w: 响应缺少 txn_id", ErrDebitUnavailable)
	}
	return DebitResult{
		TxnID:    string(data.TxnID),
		Amount:   float64(data.Amount),
		Balance:  float64(data.Balance),
		Currency: data.Currency,
	}, nil
}
