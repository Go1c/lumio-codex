package sub2api

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

const testWalletClient = "test-wallet-client"

func debitClient(base string) *Client {
	return New(Options{
		BaseURL:   base,
		ClientKey: testWalletClient,
		Sleep:     func(context.Context, time.Duration) error { return nil },
	})
}

func TestDebitReturnsTheEnvelopeResult(t *testing.T) {
	var gotBody []byte
	up := newFakeUpstream(t, func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Idempotency-Key") != "idem-1" {
			t.Errorf("Idempotency-Key = %q", r.Header.Get("Idempotency-Key"))
		}
		if r.Header.Get("Authorization") != "Bearer user-tok" {
			t.Errorf("Authorization = %q", r.Header.Get("Authorization"))
		}
		if r.Header.Get(BalanceClientHeader) != testWalletClient {
			t.Errorf("%s = %q", BalanceClientHeader, r.Header.Get(BalanceClientHeader))
		}
		if r.Header.Get("X-Forwarded-For") != "203.0.113.9" {
			t.Errorf("X-Forwarded-For = %q", r.Header.Get("X-Forwarded-For"))
		}
		gotBody, _ = io.ReadAll(r.Body)
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"code":0,"message":"ok","reason":"","data":{"txn_id":"txn-19","amount":19.90,"balance":80.10,"currency":"CNY"}}`))
	})

	got, err := debitClient(up.server.URL).Debit(
		context.Background(), "user-tok", "idem-1",
		DebitRequest{AmountCents: 1990, Currency: "CNY", Purpose: "cchaven_monthly", Ref: "CC20260819-000001", ClientIP: "203.0.113.9", UserAgent: "BestCodex/1"},
	)
	if err != nil {
		t.Fatalf("Debit() 失败: %v", err)
	}
	if !bytes.Contains(gotBody, []byte(`"amount":19.90`)) {
		t.Errorf("请求体金额应为定点 19.90, got %s", gotBody)
	}
	if bytes.Contains(gotBody, []byte("user_id")) {
		t.Error("请求体不得带 user_id")
	}
	if got.TxnID != "txn-19" || got.AmountCents != 1990 || got.BalanceCents != 8010 {
		t.Errorf("回执 = %+v", got)
	}
}

func TestDebitRequiresClientKey(t *testing.T) {
	up := newFakeUpstream(t, func(w http.ResponseWriter, _ *http.Request) {
		t.Error("未配置消费方密钥时不得打上游")
		w.WriteHeader(http.StatusOK)
	})
	_, err := New(Options{BaseURL: up.server.URL, Sleep: func(context.Context, time.Duration) error { return nil }}).
		Debit(context.Background(), "tok", "idem", DebitRequest{AmountCents: 1990, Currency: "CNY", Purpose: "cchaven_monthly", Ref: "ord-1"})
	if !errors.Is(err, ErrDebitMisconfigured) {
		t.Fatalf("err = %v, want ErrDebitMisconfigured", err)
	}
	if up.calls.Load() != 0 {
		t.Errorf("上游调用 = %d, want 0", up.calls.Load())
	}
}

func TestDebitUsesConfiguredPath(t *testing.T) {
	const custom = "/custom/balance/debit"
	up := newFakeUpstream(t, func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"code":0,"data":{"txn_id":"txn-path","amount":19.90,"balance":0.10,"currency":"CNY"}}`))
	})

	_, err := New(Options{BaseURL: up.server.URL, DebitPath: custom, ClientKey: testWalletClient, Sleep: func(context.Context, time.Duration) error { return nil }}).
		Debit(context.Background(), "tok", "idem", DebitRequest{AmountCents: 1990, Currency: "CNY", Purpose: "cchaven_monthly", Ref: "ord-1"})
	if err != nil {
		t.Fatalf("Debit() 失败: %v", err)
	}
	if up.lastPath != custom {
		t.Errorf("请求路径 = %q, want %q", up.lastPath, custom)
	}
}

func TestDebitMapsTypedReasons(t *testing.T) {
	cases := []struct {
		status int
		body   string
		want   error
	}{
		{403, `{"code":1,"reason":"INSUFFICIENT_BALANCE"}`, ErrInsufficientBalance},
		{401, `{"code":1,"reason":"TOKEN_EXPIRED"}`, ErrTokenExpired},
		{401, `{"code":401,"message":"unauthorized"}`, ErrInvalidToken},
		{429, `{"code":1,"reason":"BALANCE_DEBIT_BUSY"}`, ErrDebitBusy},
		{403, `{"code":1,"reason":"INVALID_BALANCE_CLIENT"}`, ErrInvalidBalanceClient},
		{403, `{"code":1,"reason":"PURPOSE_NOT_ALLOWED"}`, ErrPurposeNotAllowed},
		{409, `{"code":1,"reason":"IDEMPOTENCY_KEY_CONFLICT"}`, ErrIdempotencyConflict},
		{503, `{"code":1,"reason":"BALANCE_STORE_UNAVAILABLE"}`, ErrDebitUnavailable},
		{400, `{"code":1,"reason":"INVALID_AMOUNT"}`, ErrDebitInvalidRequest},
	}
	for _, tc := range cases {
		t.Run(tc.want.Error(), func(t *testing.T) {
			up := newFakeUpstream(t, func(w http.ResponseWriter, _ *http.Request) {
				w.Header().Set("Content-Type", "application/json")
				w.WriteHeader(tc.status)
				_, _ = w.Write([]byte(tc.body))
			})
			_, err := debitClient(up.server.URL).Debit(
				context.Background(), "tok", "idem",
				DebitRequest{AmountCents: 1990, Currency: "CNY", Purpose: "cchaven_monthly", Ref: "ord-1"},
			)
			if !errors.Is(err, tc.want) {
				t.Fatalf("err = %v, want %v", err, tc.want)
			}
			if debitRetryable(tc.want) {
				return
			}
			if up.calls.Load() != 1 {
				t.Errorf("不可重试错误被重试了 %d 次", up.calls.Load())
			}
		})
	}
}

func TestDebitRetriesBusyAndUnavailableWithSameBody(t *testing.T) {
	var (
		mu     sync.Mutex
		bodies []string
	)
	var n atomic.Int64
	up := newFakeUpstream(t, func(w http.ResponseWriter, r *http.Request) {
		raw, _ := io.ReadAll(r.Body)
		mu.Lock()
		bodies = append(bodies, string(raw))
		mu.Unlock()
		if n.Add(1) < 3 {
			w.Header().Set("Retry-After", "1")
			w.WriteHeader(http.StatusTooManyRequests)
			_, _ = w.Write([]byte(`{"code":1,"reason":"BALANCE_DEBIT_BUSY"}`))
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"code":0,"data":{"txn_id":"txn-busy","amount":19.90,"balance":1.00,"currency":"CNY"}}`))
	})

	got, err := debitClient(up.server.URL).Debit(
		context.Background(), "tok", "ord-1",
		DebitRequest{AmountCents: 1990, Currency: "CNY", Purpose: "cchaven_monthly", Ref: "ord-1"},
	)
	if err != nil {
		t.Fatalf("Debit() 失败: %v", err)
	}
	if got.TxnID != "txn-busy" {
		t.Errorf("TxnID = %q", got.TxnID)
	}
	if up.calls.Load() != 3 {
		t.Errorf("调用次数 = %d, want 3", up.calls.Load())
	}
	mu.Lock()
	defer mu.Unlock()
	if len(bodies) < 2 || bodies[0] != bodies[1] {
		t.Fatalf("重试请求体必须完全一致: %#v", bodies)
	}
}

func TestDebitDoesNotRetryConflictsOrConfigErrors(t *testing.T) {
	up := newFakeUpstream(t, func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusConflict)
		_, _ = w.Write([]byte(`{"code":1,"reason":"IDEMPOTENCY_KEY_CONFLICT"}`))
	})
	_, err := debitClient(up.server.URL).Debit(
		context.Background(), "tok", "idem",
		DebitRequest{AmountCents: 1990, Currency: "CNY", Purpose: "cchaven_monthly", Ref: "ord-1"},
	)
	if !errors.Is(err, ErrIdempotencyConflict) {
		t.Fatalf("err = %v", err)
	}
	if up.calls.Load() != 1 {
		t.Errorf("冲突不得自动重试, calls=%d", up.calls.Load())
	}
}

func TestDebitTreatsIdempotentReplayAsSuccess(t *testing.T) {
	var realDebits atomic.Int64
	seen := sync.Map{}
	up := newFakeUpstream(t, func(w http.ResponseWriter, r *http.Request) {
		key := r.Header.Get("Idempotency-Key")
		if _, loaded := seen.LoadOrStore(key, struct{}{}); !loaded {
			realDebits.Add(1)
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"code":0,"data":{"txn_id":"txn-same","amount":19.90,"balance":0.10,"currency":"CNY"}}`))
	})

	client := debitClient(up.server.URL)
	req := DebitRequest{AmountCents: 1990, Currency: "CNY", Purpose: "cchaven_monthly", Ref: "ord-1"}
	first, err := client.Debit(context.Background(), "tok", "same-key", req)
	if err != nil {
		t.Fatalf("第一次 Debit() 失败: %v", err)
	}
	second, err := client.Debit(context.Background(), "tok", "same-key", req)
	if err != nil {
		t.Fatalf("第二次 Debit() 失败: %v", err)
	}
	if first.TxnID != "txn-same" || second.TxnID != first.TxnID {
		t.Errorf("txn = %q / %q", first.TxnID, second.TxnID)
	}
	if realDebits.Load() != 1 {
		t.Errorf("真实扣减次数 = %d", realDebits.Load())
	}
}

func TestDebitRejectsInvalidLocalAmounts(t *testing.T) {
	up := newFakeUpstream(t, func(http.ResponseWriter, *http.Request) {
		t.Error("非法金额不得发请求")
	})
	for _, cents := range []int64{0, -1990} {
		_, err := debitClient(up.server.URL).Debit(
			context.Background(), "tok", "idem",
			DebitRequest{AmountCents: cents, Currency: "CNY", Purpose: "cchaven_monthly", Ref: "ord-1"},
		)
		if !errors.Is(err, ErrDebitInvalidRequest) {
			t.Errorf("cents=%d err=%v", cents, err)
		}
	}
	if up.calls.Load() != 0 {
		t.Errorf("上游调用 = %d", up.calls.Load())
	}
}

func TestDebitRejectsMismatchedReceipt(t *testing.T) {
	cases := []string{
		`{"code":0,"data":{"txn_id":"","amount":19.90,"balance":0,"currency":"CNY"}}`,
		`{"code":0,"data":{"txn_id":"txn-x","amount":1.99,"balance":0,"currency":"CNY"}}`,
		`{"code":0,"data":{"txn_id":"txn-x","amount":19.90,"balance":0,"currency":"USD"}}`,
		`{"code":0,"data":{"txn_id":"txn-x","amount":"19.90","balance":0,"currency":"CNY"}}`,
	}
	for _, body := range cases {
		up := newFakeUpstream(t, func(w http.ResponseWriter, _ *http.Request) {
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(body))
		})
		_, err := debitClient(up.server.URL).Debit(
			context.Background(), "tok", "idem",
			DebitRequest{AmountCents: 1990, Currency: "CNY", Purpose: "cchaven_monthly", Ref: "ord-1"},
		)
		if !errors.Is(err, ErrDebitUnavailable) {
			t.Errorf("body=%s err=%v", body, err)
		}
	}
}

func TestParseDebitAcceptsScale8WalletBalance(t *testing.T) {
	body := []byte(`{"code":0,"message":"success","data":{"txn_id":"txn-scale","amount":19.90,"balance":583.46000000,"currency":"CNY"}}`)
	got, err := parseDebit(http.StatusOK, body, DebitRequest{
		AmountCents: 1990, Currency: "CNY", Purpose: "cchaven_monthly", Ref: "ord-1",
	})
	if err != nil {
		t.Fatalf("HTTP 200 + code=0 + 8 位小数余额不得变成 ErrDebitUnavailable, err=%v", err)
	}
	if got.TxnID != "txn-scale" || got.AmountCents != 1990 || got.BalanceCents != 58346 {
		t.Errorf("回执 = %+v, want txn-scale / 1990 / 58346", got)
	}
}

func TestDebitPayloadHasNoSecretOrFloat(t *testing.T) {
	payload, err := marshalDebitRequest(DebitRequest{
		AmountCents: 1990, Currency: "CNY", Purpose: "cchaven_monthly", Ref: "CC1",
	})
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(payload, []byte(testWalletClient)) || bytes.Contains(bytes.ToLower(payload), []byte("bcs_")) {
		t.Fatalf("请求体泄漏了消费方密钥: %s", payload)
	}
	var decoded map[string]json.RawMessage
	if err := json.Unmarshal(payload, &decoded); err != nil {
		t.Fatal(err)
	}
	if string(decoded["amount"]) != "19.90" {
		t.Errorf("amount JSON = %s", decoded["amount"])
	}
}

func TestVerifyFreshBypassesTheCacheAndWritesBack(t *testing.T) {
	var body atomic.Value
	body.Store(`{"data":{"id":"1","email":"old@x","balance":1,"status":"active"}}`)

	up := newFakeUpstream(t, func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(body.Load().(string)))
	})

	clock := time.Now()
	client := New(Options{
		BaseURL:  up.server.URL,
		CacheTTL: time.Minute,
		Now:      func() time.Time { return clock },
	})

	first, err := client.Verify(context.Background(), "tok")
	if err != nil {
		t.Fatalf("Verify() 失败: %v", err)
	}
	if first.Email != "old@x" {
		t.Fatalf("首次 Email = %q", first.Email)
	}

	body.Store(`{"data":{"id":"1","email":"new@x","balance":19.9,"status":"active"}}`)

	cached, err := client.Verify(context.Background(), "tok")
	if err != nil {
		t.Fatalf("缓存 Verify() 失败: %v", err)
	}
	if cached.Email != "old@x" {
		t.Errorf("TTL 内 Verify 应读缓存, Email = %q", cached.Email)
	}

	fresh, err := client.VerifyFresh(context.Background(), "tok")
	if err != nil {
		t.Fatalf("VerifyFresh() 失败: %v", err)
	}
	if fresh.Email != "new@x" {
		t.Errorf("VerifyFresh = %+v", fresh)
	}
}
