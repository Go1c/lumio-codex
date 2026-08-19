package testsupport

import (
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/sub2api"
)

// FakeSub2API 是测试用的假账号中心。
//
// 真的 Sub2API 不在本仓库里，集成测试又必须覆盖「身份来自外部」这条主链路，
// 因此这里实现被依赖的端点：GET /api/v1/auth/me 与 POST 余额扣费。
type FakeSub2API struct {
	server *httptest.Server

	mu          sync.Mutex
	byToken     map[string]sub2api.Identity
	nextID      int64
	nextTxn     int64
	unavailable bool
	debitByKey  map[string]sub2api.DebitResult
	debitCalls  []DebitCall
}

// DebitCall 是一次打到假扣费端点的请求记录。
type DebitCall struct {
	Token          string
	IdempotencyKey string
	Amount         float64
	Currency       string
	Purpose        string
	Ref            string
}

// NewFakeSub2API 启动假账号中心，随测试结束自动关闭。
func NewFakeSub2API(t *testing.T) *FakeSub2API {
	t.Helper()

	fake := &FakeSub2API{
		byToken:    map[string]sub2api.Identity{},
		debitByKey: map[string]sub2api.DebitResult{},
		nextID:     900000,
	}
	fake.server = httptest.NewServer(http.HandlerFunc(fake.handle))
	t.Cleanup(fake.server.Close)
	return fake
}

// URL 返回假账号中心的基地址。
func (f *FakeSub2API) URL() string { return f.server.URL }

func (f *FakeSub2API) handle(w http.ResponseWriter, r *http.Request) {
	f.mu.Lock()
	defer f.mu.Unlock()

	if f.unavailable {
		w.WriteHeader(http.StatusInternalServerError)
		return
	}
	switch {
	case r.URL.Path == sub2api.MePath:
		f.handleMe(w, r)
	case r.Method == http.MethodPost && r.URL.Path == sub2api.DefaultDebitPath:
		f.handleDebit(w, r)
	default:
		w.WriteHeader(http.StatusNotFound)
	}
}

func (f *FakeSub2API) handleMe(w http.ResponseWriter, r *http.Request) {
	token := strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer ")
	identity, ok := f.byToken[strings.TrimSpace(token)]
	if !ok {
		w.WriteHeader(http.StatusUnauthorized)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(map[string]any{"data": identity})
}

func (f *FakeSub2API) handleDebit(w http.ResponseWriter, r *http.Request) {
	token := strings.TrimSpace(strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer "))
	identity, ok := f.byToken[token]
	if !ok {
		w.WriteHeader(http.StatusUnauthorized)
		return
	}

	var req sub2api.DebitRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		w.WriteHeader(http.StatusBadRequest)
		return
	}

	key := strings.TrimSpace(r.Header.Get("Idempotency-Key"))
	f.debitCalls = append(f.debitCalls, DebitCall{
		Token: token, IdempotencyKey: key,
		Amount: req.Amount, Currency: req.Currency, Purpose: req.Purpose, Ref: req.Ref,
	})

	if key != "" {
		if replay, hit := f.debitByKey[idempotencySlot(token, key)]; hit {
			writeDebitOK(w, replay)
			return
		}
	}

	if identity.Balance < req.Amount {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusForbidden)
		_ = json.NewEncoder(w).Encode(map[string]any{
			"code": 403, "message": "余额不足", "reason": "INSUFFICIENT_BALANCE",
		})
		return
	}

	f.nextTxn++
	identity.Balance -= req.Amount
	f.byToken[token] = identity
	result := sub2api.DebitResult{
		TxnID:    fmt.Sprintf("txn-%d", f.nextTxn),
		Amount:   req.Amount,
		Balance:  identity.Balance,
		Currency: req.Currency,
	}
	if result.Currency == "" {
		result.Currency = "CNY"
	}
	if key != "" {
		f.debitByKey[idempotencySlot(token, key)] = result
	}
	writeDebitOK(w, result)
}

func writeDebitOK(w http.ResponseWriter, result sub2api.DebitResult) {
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(map[string]any{
		"code": 0, "message": "success", "data": result,
	})
}

func idempotencySlot(token, key string) string {
	return token + "\x00" + key
}

// Issue 在账号中心建一个用户并返回它的 access token。
func (f *FakeSub2API) Issue(email string) string {
	f.mu.Lock()
	defer f.mu.Unlock()

	f.nextID++
	id := fmt.Sprintf("%d", f.nextID)
	return f.issueLocked(id, email)
}

// IssueFor 为指定的 Sub2API 用户 ID 再签一个令牌，
// 用于验证「同一个人换设备 / 换令牌仍映射到同一个本地账号」。
func (f *FakeSub2API) IssueFor(sub2apiUserID, email string) string {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.issueLocked(sub2apiUserID, email)
}

func (f *FakeSub2API) issueLocked(id, email string) string {
	token := fmt.Sprintf("s2a-%s-%d", id, len(f.byToken)+1)
	f.byToken[token] = sub2api.Identity{ID: id, Email: email, Status: "active"}
	return token
}

// UserIDOf 返回令牌对应的 Sub2API 用户 ID。
func (f *FakeSub2API) UserIDOf(token string) string {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.byToken[token].ID
}

// SetEmail 模拟用户在账号中心改了邮箱。
func (f *FakeSub2API) SetEmail(token, email string) {
	f.mu.Lock()
	defer f.mu.Unlock()

	identity := f.byToken[token]
	identity.Email = email
	f.byToken[token] = identity
}

// SetStatus 模拟账号在账号中心被停用。
func (f *FakeSub2API) SetStatus(token, status string) {
	f.mu.Lock()
	defer f.mu.Unlock()

	identity := f.byToken[token]
	identity.Status = status
	f.byToken[token] = identity
}

// Revoke 让令牌失效，之后校验一律 401。
func (f *FakeSub2API) Revoke(token string) {
	f.mu.Lock()
	defer f.mu.Unlock()
	delete(f.byToken, token)
}

// SetUnavailable 模拟账号中心故障，用于验证降级策略（必须 503，不得放行）。
func (f *FakeSub2API) SetUnavailable(down bool) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.unavailable = down
}

// SetBalance 设定令牌对应用户的账户余额（元）。
func (f *FakeSub2API) SetBalance(token string, balance float64) {
	f.mu.Lock()
	defer f.mu.Unlock()

	identity := f.byToken[token]
	identity.Balance = balance
	f.byToken[token] = identity
}

// BalanceOf 返回令牌对应用户的当前余额。
func (f *FakeSub2API) BalanceOf(token string) float64 {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.byToken[token].Balance
}

// DebitCalls 返回打到扣费端点的请求副本。
func (f *FakeSub2API) DebitCalls() []DebitCall {
	f.mu.Lock()
	defer f.mu.Unlock()
	out := make([]DebitCall, len(f.debitCalls))
	copy(out, f.debitCalls)
	return out
}
