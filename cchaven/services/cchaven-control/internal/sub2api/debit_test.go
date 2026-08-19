package sub2api

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func TestDebitReturnsTheEnvelopeResult(t *testing.T) {
	up := newFakeUpstream(t, func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			t.Errorf("method = %s, want POST", r.Method)
		}
		if r.Header.Get("Idempotency-Key") != "idem-1" {
			t.Errorf("Idempotency-Key = %q", r.Header.Get("Idempotency-Key"))
		}
		if r.Header.Get("Authorization") != "Bearer user-tok" {
			t.Errorf("Authorization = %q", r.Header.Get("Authorization"))
		}

		var body DebitRequest
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
			t.Fatalf("解析请求体失败: %v", err)
		}
		if body.Amount != 19.9 {
			t.Errorf("amount = %v, want 19.9", body.Amount)
		}
		if body.Currency != "CNY" {
			t.Errorf("currency = %q", body.Currency)
		}
		if body.Purpose != "cchaven_monthly" {
			t.Errorf("purpose = %q", body.Purpose)
		}
		if body.Ref != "CC20260819-000001" {
			t.Errorf("ref = %q", body.Ref)
		}

		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"code":0,"message":"success","data":{"txn_id":"txn-19","amount":19.9,"balance":80.1,"currency":"CNY"}}`))
	})

	got, err := New(Options{BaseURL: up.server.URL}).Debit(
		context.Background(), "user-tok", "idem-1",
		DebitRequest{Amount: 19.9, Currency: "CNY", Purpose: "cchaven_monthly", Ref: "CC20260819-000001"},
	)
	if err != nil {
		t.Fatalf("Debit() 失败: %v", err)
	}
	if up.lastPath != DefaultDebitPath {
		t.Errorf("请求路径 = %q, want %q", up.lastPath, DefaultDebitPath)
	}
	if got.TxnID != "txn-19" {
		t.Errorf("TxnID = %q, want txn-19", got.TxnID)
	}
	if got.Amount != 19.9 {
		t.Errorf("Amount = %v, want 19.9", got.Amount)
	}
	if got.Balance != 80.1 {
		t.Errorf("Balance = %v, want 80.1", got.Balance)
	}
	if got.Currency != "CNY" {
		t.Errorf("Currency = %q", got.Currency)
	}
}

func TestDebitMapsInsufficientBalance(t *testing.T) {
	up := newFakeUpstream(t, func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusForbidden)
		_, _ = w.Write([]byte(`{"code":403,"message":"余额不足","reason":"INSUFFICIENT_BALANCE"}`))
	})

	_, err := New(Options{BaseURL: up.server.URL}).Debit(
		context.Background(), "tok", "idem",
		DebitRequest{Amount: 19.9, Currency: "CNY", Purpose: "cchaven_monthly", Ref: "ord-1"},
	)
	if !errors.Is(err, ErrInsufficientBalance) {
		t.Fatalf("err = %v, want ErrInsufficientBalance", err)
	}
}

// TestDebitTreatsIdempotentReplayAsSuccess 锁住幂等语义：
// 同一 Idempotency-Key 两次调用只产生一次真实扣减，第二次 200 同一 txn_id 也算成功。
func TestDebitTreatsIdempotentReplayAsSuccess(t *testing.T) {
	var (
		mu         sync.Mutex
		seen       = map[string]struct{}{}
		realDebits int
	)
	up := newFakeUpstream(t, func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		key := r.Header.Get("Idempotency-Key")
		if _, ok := seen[key]; !ok {
			seen[key] = struct{}{}
			realDebits++
		}
		mu.Unlock()

		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"code":0,"data":{"txn_id":"txn-same","amount":19.9,"balance":0.1,"currency":"CNY"}}`))
	})

	client := New(Options{BaseURL: up.server.URL})
	req := DebitRequest{Amount: 19.9, Currency: "CNY", Purpose: "cchaven_monthly", Ref: "ord-1"}

	first, err := client.Debit(context.Background(), "tok", "same-key", req)
	if err != nil {
		t.Fatalf("第一次 Debit() 失败: %v", err)
	}
	second, err := client.Debit(context.Background(), "tok", "same-key", req)
	if err != nil {
		t.Fatalf("第二次 Debit() 失败: %v", err)
	}

	if first.TxnID != "txn-same" || second.TxnID != first.TxnID {
		t.Errorf("txn = %q / %q, 两次应是同一 txn_id", first.TxnID, second.TxnID)
	}
	mu.Lock()
	gotReal := realDebits
	mu.Unlock()
	if gotReal != 1 {
		t.Errorf("真实扣减次数 = %d, want 1", gotReal)
	}
	if got := up.calls.Load(); got != 2 {
		t.Errorf("HTTP 调用次数 = %d, want 2（客户端每次都发，幂等由上游保证）", got)
	}
}

func TestDebitMapsUpstreamFailureToUnavailable(t *testing.T) {
	cases := []struct {
		name   string
		status int
	}{
		{"上游 500", http.StatusInternalServerError},
		{"上游 502", http.StatusBadGateway},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			up := newFakeUpstream(t, func(w http.ResponseWriter, _ *http.Request) {
				w.WriteHeader(tc.status)
			})

			_, err := New(Options{BaseURL: up.server.URL}).Debit(
				context.Background(), "tok", "idem",
				DebitRequest{Amount: 19.9, Currency: "CNY", Purpose: "cchaven_monthly", Ref: "ord-1"},
			)
			if !errors.Is(err, ErrDebitUnavailable) {
				t.Fatalf("err = %v, want ErrDebitUnavailable", err)
			}
		})
	}
}

// TestVerifyFreshBypassesTheCacheAndWritesBack 锁住扣费前必须回源：
// VerifyFresh 不得只读旧缓存，打完上游后要把新快照写回，后续 Verify 才能看到新余额。
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
	if got := up.calls.Load(); got != 1 {
		t.Fatalf("缓存命中后调用次数 = %d, want 1", got)
	}

	fresh, err := client.VerifyFresh(context.Background(), "tok")
	if err != nil {
		t.Fatalf("VerifyFresh() 失败: %v", err)
	}
	if fresh.Email != "new@x" || fresh.Balance != 19.9 {
		t.Errorf("VerifyFresh = %+v, 应打上游拿到新快照", fresh)
	}
	if got := up.calls.Load(); got != 2 {
		t.Fatalf("VerifyFresh 必须打上游, 调用次数 = %d, want 2", got)
	}

	rewritten, err := client.Verify(context.Background(), "tok")
	if err != nil {
		t.Fatalf("回写后 Verify() 失败: %v", err)
	}
	if rewritten.Email != "new@x" {
		t.Errorf("VerifyFresh 应写回缓存, Email = %q", rewritten.Email)
	}
	if got := up.calls.Load(); got != 2 {
		t.Errorf("写回后 Verify 不应再打上游, 调用次数 = %d", got)
	}
}
