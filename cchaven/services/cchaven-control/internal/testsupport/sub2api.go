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

	mu                      sync.Mutex
	byToken                 map[string]sub2api.Identity
	nextID                  int64
	nextTxn                 int64
	unavailable             bool
	debitUnavailable        bool
	debitNextReason         string
	debitNextStatus         int
	debitByKey              map[string]sub2api.DebitResult
	debitCalls              []DebitCall
	debitCharges            int
	debitBalanceScale8      bool
	debitReceiptUnparseable bool
}

// DebitCall 是一次打到假扣费端点的请求记录。
type DebitCall struct {
	Token          string
	IdempotencyKey string
	AmountCents    int64
	Currency       string
	Purpose        string
	Ref            string
	ClientKey      string
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
		if f.debitUnavailable {
			w.WriteHeader(http.StatusBadGateway)
			return
		}
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

	if f.debitNextReason != "" {
		status := f.debitNextStatus
		reason := f.debitNextReason
		f.debitNextReason = ""
		f.debitNextStatus = 0
		if status == 0 {
			status = http.StatusUnauthorized
		}
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(status)
		_ = json.NewEncoder(w).Encode(map[string]any{"code": status, "reason": reason})
		return
	}

	var raw struct {
		Amount   json.RawMessage `json:"amount"`
		Currency string          `json:"currency"`
		Purpose  string          `json:"purpose"`
		Ref      string          `json:"ref"`
	}
	if err := json.NewDecoder(r.Body).Decode(&raw); err != nil {
		w.WriteHeader(http.StatusBadRequest)
		return
	}
	amountCents, err := sub2api.ParseYuanJSON(raw.Amount)
	if err != nil {
		w.WriteHeader(http.StatusBadRequest)
		return
	}

	key := strings.TrimSpace(r.Header.Get("Idempotency-Key"))
	f.debitCalls = append(f.debitCalls, DebitCall{
		Token: token, IdempotencyKey: key,
		AmountCents: amountCents, Currency: raw.Currency, Purpose: raw.Purpose, Ref: raw.Ref,
		ClientKey: r.Header.Get(sub2api.BalanceClientHeader),
	})

	if key != "" {
		if replay, hit := f.debitByKey[idempotencySlot(token, key)]; hit {
			f.writeDebitOK(w, replay)
			return
		}
	}

	if identity.Balance*100 < float64(amountCents) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusForbidden)
		_ = json.NewEncoder(w).Encode(map[string]any{
			"code": 403, "message": "余额不足", "reason": "INSUFFICIENT_BALANCE",
		})
		return
	}

	f.nextTxn++
	identity.Balance -= float64(amountCents) / 100
	f.byToken[token] = identity
	result := sub2api.DebitResult{
		TxnID:        fmt.Sprintf("txn-%d", f.nextTxn),
		AmountCents:  amountCents,
		BalanceCents: int64(identity.Balance*100 + 0.5),
		Currency:     raw.Currency,
	}
	if result.Currency == "" {
		result.Currency = "CNY"
	}
	if key != "" {
		f.debitByKey[idempotencySlot(token, key)] = result
	}
	f.debitCharges++
	f.writeDebitOK(w, result)
}

func (f *FakeSub2API) writeDebitOK(w http.ResponseWriter, result sub2api.DebitResult) {
	amount, err := sub2api.FormatYuan(result.AmountCents)
	if err != nil {
		amount = "0.00"
	}
	balance := "0.00"
	if result.BalanceCents > 0 {
		if formatted, ferr := sub2api.FormatYuan(result.BalanceCents); ferr == nil {
			balance = formatted
		}
	}
	if f.debitBalanceScale8 {
		balance = padYuanScale8(balance)
	}
	w.Header().Set("Content-Type", "application/json")
	if f.debitReceiptUnparseable {
		// 模拟回执缺 balance / 快照解不开。真实扣款已经发生，控制面必须仍能入账。
		_, _ = fmt.Fprintf(w, `{"code":0,"message":"success","data":{"txn_id":%q,"amount":%s,"currency":%q}}`,
			result.TxnID, amount, result.Currency)
		return
	}
	_, _ = fmt.Fprintf(w, `{"code":0,"message":"success","data":{"txn_id":%q,"amount":%s,"balance":%s,"currency":%q}}`,
		result.TxnID, amount, balance, result.Currency)
}

func padYuanScale8(value string) string {
	whole, frac, dotted := strings.Cut(value, ".")
	if !dotted {
		return whole + ".00000000"
	}
	for len(frac) < 8 {
		frac += "0"
	}
	return whole + "." + frac
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

// SetDebitUnavailable 只让扣费端点失败，/auth/me 仍可用。
func (f *FakeSub2API) SetDebitUnavailable(down bool) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.debitUnavailable = down
}

// FailNextDebit 让下一次扣费返回指定 reason（不含密钥）。
func (f *FakeSub2API) FailNextDebit(status int, reason string) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.debitNextStatus = status
	f.debitNextReason = reason
}

// SetDebitBalanceScale8 让成功回执的余额带 numeric(20,8) 尾零。
func (f *FakeSub2API) SetDebitBalanceScale8(on bool) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.debitBalanceScale8 = on
}

// SetDebitReceiptUnparseable 让成功回执缺 balance，模拟快照解不开。
// 真实扣款仍会发生；同一 order_no 重放不得再扣。
func (f *FakeSub2API) SetDebitReceiptUnparseable(on bool) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.debitReceiptUnparseable = on
}

// DebitChargeCount 是真正改余额的次数，不含幂等重放。
func (f *FakeSub2API) DebitChargeCount() int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.debitCharges
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
