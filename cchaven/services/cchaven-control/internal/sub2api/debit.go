package sub2api

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"strconv"
	"strings"
	"time"
)

const (
	// DefaultDebitPath 是用账户余额扣一笔费用的端点。
	DefaultDebitPath = "/api/v1/user/balance/debit"
	// BalanceClientHeader 是消费方身份头，值只能来自服务端 secret。
	BalanceClientHeader = "X-Balance-Client-Key"
	maxDebitAttempts    = 4
	maxDebitBackoff     = 2 * time.Second
)

var (
	// ErrInsufficientBalance 表示账户余额不够支付这一笔。
	ErrInsufficientBalance = errors.New("sub2api: 余额不足")
	// ErrDebitUnavailable 表示扣费接口暂时不可用，可用原请求重试。
	ErrDebitUnavailable = errors.New("sub2api: 扣费不可用")
	// ErrTokenExpired 表示用户 JWT 过期，调用方刷新后再用原订单重试。
	ErrTokenExpired = errors.New("sub2api: 令牌过期")
	// ErrDebitBusy 表示上游正在处理同一笔扣款，需按 Retry-After 有界重试。
	ErrDebitBusy = errors.New("sub2api: 扣款忙")
	// ErrInvalidBalanceClient 表示消费方密钥不被接受，停止自动重试。
	ErrInvalidBalanceClient = errors.New("sub2api: 消费方密钥无效")
	// ErrPurposeNotAllowed 表示 purpose 未被批准，停止自动重试。
	ErrPurposeNotAllowed = errors.New("sub2api: purpose 不被允许")
	// ErrIdempotencyConflict 表示同一 key 的请求体与首次不一致，停止重试。
	ErrIdempotencyConflict = errors.New("sub2api: 幂等键冲突")
	// ErrDebitInvalidRequest 表示请求校验失败，应修实现而不是重试。
	ErrDebitInvalidRequest = errors.New("sub2api: 扣款请求不合法")
	// ErrDebitMisconfigured 表示本服务未配消费方密钥。
	ErrDebitMisconfigured = errors.New("sub2api: 未配置消费方密钥")
)

// DebitRequest 是一笔站内余额扣费。金额只用分，编码时再写成十进制文本。
type DebitRequest struct {
	AmountCents int64
	Currency    string
	Purpose     string
	Ref         string
	ClientIP    string
	UserAgent   string
}

// DebitResult 是上游确认入账后的回执。金额同样以分为单位。
type DebitResult struct {
	TxnID        string
	AmountCents  int64
	BalanceCents int64
	Currency     string
}

// DebitBusyError 带上游建议的等待时间。
type DebitBusyError struct {
	RetryAfter time.Duration
}

func (e *DebitBusyError) Error() string { return ErrDebitBusy.Error() }
func (e *DebitBusyError) Unwrap() error { return ErrDebitBusy }

// Debit 用当前用户 JWT 从 LumioAPI 余额里扣一笔。
//
// 不发送 user_id。消费方身份只走 X-Balance-Client-Key。
// BUSY / 存储不可用 / 网络超时会按同一把 Idempotency-Key 与同一请求体有界重试。
func (c *Client) Debit(ctx context.Context, userToken string, idempotencyKey string, req DebitRequest) (DebitResult, error) {
	userToken = strings.TrimSpace(userToken)
	if userToken == "" {
		return DebitResult{}, ErrInvalidToken
	}
	if strings.TrimSpace(c.clientKey) == "" {
		return DebitResult{}, ErrDebitMisconfigured
	}
	payload, err := marshalDebitRequest(req)
	if err != nil {
		return DebitResult{}, err
	}

	var last error
	for attempt := 0; attempt < maxDebitAttempts; attempt++ {
		result, retryAfter, err := c.debitOnce(ctx, userToken, idempotencyKey, payload, req)
		if err == nil {
			return result, nil
		}
		last = err
		if !debitRetryable(err) || attempt == maxDebitAttempts-1 {
			return DebitResult{}, err
		}
		wait := debitBackoff(attempt, retryAfter)
		if err := c.sleep(ctx, wait); err != nil {
			return DebitResult{}, fmt.Errorf("%w: %v", ErrDebitUnavailable, err)
		}
	}
	return DebitResult{}, last
}

func (c *Client) debitOnce(
	ctx context.Context, userToken, idempotencyKey string, payload []byte, expected DebitRequest,
) (DebitResult, time.Duration, error) {
	httpReq, err := http.NewRequestWithContext(ctx, http.MethodPost, c.baseURL+c.debitPath, bytes.NewReader(payload))
	if err != nil {
		return DebitResult{}, 0, fmt.Errorf("%w: 构造请求失败", ErrDebitUnavailable)
	}
	httpReq.Header.Set("Authorization", "Bearer "+userToken)
	httpReq.Header.Set("Accept", "application/json")
	httpReq.Header.Set("Content-Type", "application/json")
	httpReq.Header.Set("Idempotency-Key", strings.TrimSpace(idempotencyKey))
	httpReq.Header.Set(BalanceClientHeader, c.clientKey)
	if ip := strings.TrimSpace(expected.ClientIP); ip != "" {
		httpReq.Header.Set("X-Forwarded-For", ip)
		httpReq.Header.Set("X-Real-IP", ip)
	}
	if ua := strings.TrimSpace(expected.UserAgent); ua != "" {
		httpReq.Header.Set("User-Agent", ua)
	}

	resp, err := c.http.Do(httpReq)
	if err != nil {
		return DebitResult{}, 0, fmt.Errorf("%w: 网络错误", ErrDebitUnavailable)
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(io.LimitReader(resp.Body, maxBodyBytes))
	if err != nil {
		return DebitResult{}, 0, fmt.Errorf("%w: 读取响应失败", ErrDebitUnavailable)
	}
	retryAfter := parseRetryAfter(resp.Header.Get("Retry-After"))
	result, err := parseDebit(resp.StatusCode, body, expected)
	return result, retryAfter, err
}

func marshalDebitRequest(req DebitRequest) ([]byte, error) {
	amount, err := FormatYuan(req.AmountCents)
	if err != nil {
		return nil, fmt.Errorf("%w: %v", ErrDebitInvalidRequest, err)
	}
	currency := strings.TrimSpace(req.Currency)
	if currency == "" {
		currency = "CNY"
	}
	if !strings.EqualFold(currency, "CNY") {
		return nil, fmt.Errorf("%w: currency 必须是 CNY", ErrDebitInvalidRequest)
	}
	purpose := strings.TrimSpace(req.Purpose)
	ref := strings.TrimSpace(req.Ref)
	if purpose == "" || ref == "" {
		return nil, fmt.Errorf("%w: purpose 与 ref 必填", ErrDebitInvalidRequest)
	}
	// 金额用 []byte 拼成 JSON 数字，避免 encoding/json 走 float64。
	var buf bytes.Buffer
	buf.WriteString(`{"amount":`)
	buf.WriteString(amount)
	buf.WriteString(`,"currency":`)
	enc, _ := json.Marshal(currency)
	buf.Write(enc)
	buf.WriteString(`,"purpose":`)
	enc, _ = json.Marshal(purpose)
	buf.Write(enc)
	buf.WriteString(`,"ref":`)
	enc, _ = json.Marshal(ref)
	buf.Write(enc)
	buf.WriteByte('}')
	return buf.Bytes(), nil
}

func parseDebit(status int, raw []byte, expected DebitRequest) (DebitResult, error) {
	var envelope struct {
		Code    int             `json:"code"`
		Message string          `json:"message"`
		Reason  string          `json:"reason"`
		Data    json.RawMessage `json:"data"`
	}
	_ = json.Unmarshal(raw, &envelope)
	reason := strings.TrimSpace(envelope.Reason)

	switch {
	case reason == "INSUFFICIENT_BALANCE":
		return DebitResult{}, ErrInsufficientBalance
	case reason == "TOKEN_EXPIRED", status == http.StatusUnauthorized:
		if reason == "TOKEN_EXPIRED" {
			return DebitResult{}, ErrTokenExpired
		}
		return DebitResult{}, ErrInvalidToken
	case reason == "BALANCE_DEBIT_BUSY":
		return DebitResult{}, ErrDebitBusy
	case reason == "INVALID_BALANCE_CLIENT":
		return DebitResult{}, ErrInvalidBalanceClient
	case reason == "PURPOSE_NOT_ALLOWED":
		return DebitResult{}, ErrPurposeNotAllowed
	case reason == "IDEMPOTENCY_KEY_CONFLICT":
		return DebitResult{}, ErrIdempotencyConflict
	case reason == "BALANCE_STORE_UNAVAILABLE":
		return DebitResult{}, ErrDebitUnavailable
	case status == http.StatusBadRequest:
		return DebitResult{}, ErrDebitInvalidRequest
	}

	if status != http.StatusOK || envelope.Code != 0 {
		if status >= 500 || status == http.StatusTooManyRequests {
			return DebitResult{}, fmt.Errorf("%w: HTTP %d", ErrDebitUnavailable, status)
		}
		return DebitResult{}, fmt.Errorf("%w: HTTP %d", ErrDebitInvalidRequest, status)
	}

	var data struct {
		TxnID    flexString      `json:"txn_id"`
		Amount   json.RawMessage `json:"amount"`
		Balance  json.RawMessage `json:"balance"`
		Currency string          `json:"currency"`
	}
	if err := json.Unmarshal(envelope.Data, &data); err != nil {
		return DebitResult{}, fmt.Errorf("%w: 响应无法解析", ErrDebitUnavailable)
	}
	if strings.TrimSpace(string(data.TxnID)) == "" {
		return DebitResult{}, fmt.Errorf("%w: 响应缺少 txn_id", ErrDebitUnavailable)
	}
	amountCents, err := ParseYuanJSON(data.Amount)
	if err != nil || amountCents != expected.AmountCents {
		return DebitResult{}, fmt.Errorf("%w: 回执金额与请求不符", ErrDebitUnavailable)
	}
	// 余额只是快照。txn_id 与金额已对上时，解不开或为 0 不得把整笔打成扣费不可用。
	balanceCents, err := ParseYuanSnapshotJSON(data.Balance)
	if err != nil {
		slog.Warn("sub2api debit 回执余额无法解析，按 0 分快照继续",
			"balance", rawBalanceSnippet(data.Balance))
		balanceCents = 0
	}
	if !strings.EqualFold(strings.TrimSpace(data.Currency), "CNY") {
		return DebitResult{}, fmt.Errorf("%w: 回执币种与请求不符", ErrDebitUnavailable)
	}
	return DebitResult{
		TxnID:        string(data.TxnID),
		AmountCents:  amountCents,
		BalanceCents: balanceCents,
		Currency:     "CNY",
	}, nil
}

func rawBalanceSnippet(raw json.RawMessage) string {
	trimmed := bytes.TrimSpace(raw)
	if len(trimmed) == 0 {
		return "<missing>"
	}
	const max = 128
	if len(trimmed) > max {
		return string(trimmed[:max])
	}
	return string(trimmed)
}

func debitRetryable(err error) bool {
	return errors.Is(err, ErrDebitBusy) || errors.Is(err, ErrDebitUnavailable)
}

func debitBackoff(attempt int, retryAfter time.Duration) time.Duration {
	if retryAfter > 0 {
		if retryAfter > maxDebitBackoff {
			return maxDebitBackoff
		}
		return retryAfter
	}
	wait := 200 * time.Millisecond << attempt
	if wait > maxDebitBackoff {
		return maxDebitBackoff
	}
	return wait
}

func parseRetryAfter(raw string) time.Duration {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return 0
	}
	seconds, err := strconv.Atoi(raw)
	if err != nil || seconds <= 0 {
		return 0
	}
	return time.Duration(seconds) * time.Second
}

func (c *Client) sleep(ctx context.Context, d time.Duration) error {
	if c.sleepFn != nil {
		return c.sleepFn(ctx, d)
	}
	timer := time.NewTimer(d)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-timer.C:
		return nil
	}
}
