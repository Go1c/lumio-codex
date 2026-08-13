package sub2api

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"sync/atomic"
	"testing"
	"time"
)

// fakeUpstream 起一个假的 Sub2API，记录被调用次数与最后一次收到的请求。
type fakeUpstream struct {
	server *httptest.Server
	calls  atomic.Int64

	lastPath string
	lastAuth string
}

func newFakeUpstream(t *testing.T, handler func(w http.ResponseWriter, r *http.Request)) *fakeUpstream {
	t.Helper()

	up := &fakeUpstream{}
	up.server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		up.calls.Add(1)
		up.lastPath = r.URL.Path
		up.lastAuth = r.Header.Get("Authorization")
		handler(w, r)
	}))
	t.Cleanup(up.server.Close)
	return up
}

func respondJSON(body string) func(http.ResponseWriter, *http.Request) {
	return func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(body))
	}
}

func TestVerifyReturnsTheIdentity(t *testing.T) {
	up := newFakeUpstream(t, respondJSON(
		`{"data":{"id":"42","email":"Mary@Example.com","balance":12.5,"status":"active"}}`))

	identity, err := New(Options{BaseURL: up.server.URL}).Verify(context.Background(), "tok")
	if err != nil {
		t.Fatalf("Verify() 失败: %v", err)
	}

	if identity.ID != "42" {
		t.Errorf("ID = %q, want 42", identity.ID)
	}
	// 邮箱大小写由 Sub2API 决定，这里必须原样透传，归一化留给调用方。
	if identity.Email != "Mary@Example.com" {
		t.Errorf("Email = %q", identity.Email)
	}
	if identity.Balance != 12.5 {
		t.Errorf("Balance = %v, want 12.5", identity.Balance)
	}
	if !identity.Active() {
		t.Error("status=active 应判定为可用账号")
	}
	if up.lastPath != MePath {
		t.Errorf("请求路径 = %q, want %q", up.lastPath, MePath)
	}
	if up.lastAuth != "Bearer tok" {
		t.Errorf("Authorization = %q, want Bearer tok", up.lastAuth)
	}
}

// TestVerifyAcceptsAFlatBody 兼容不带 data 信封、且 id 为数字的返回。
// 上游的信封形态不在本仓库控制之下，解析必须容得下这两种写法。
func TestVerifyAcceptsAFlatBody(t *testing.T) {
	up := newFakeUpstream(t, respondJSON(`{"id":7,"email":"a@b.c","balance":"3.5","status":"active"}`))

	identity, err := New(Options{BaseURL: up.server.URL}).Verify(context.Background(), "tok")
	if err != nil {
		t.Fatalf("Verify() 失败: %v", err)
	}
	if identity.ID != "7" {
		t.Errorf("ID = %q, want 7", identity.ID)
	}
	if identity.Balance != 3.5 {
		t.Errorf("Balance = %v, want 3.5", identity.Balance)
	}
}

func TestVerifyRejectsAResponseWithoutAnID(t *testing.T) {
	up := newFakeUpstream(t, respondJSON(`{"data":{"email":"a@b.c"}}`))

	_, err := New(Options{BaseURL: up.server.URL}).Verify(context.Background(), "tok")
	// 没有用户 ID 就无法定位本地账号，宁可判上游异常也不能放行。
	if !errors.Is(err, ErrUnavailable) {
		t.Fatalf("缺少 id 时 err = %v, want ErrUnavailable", err)
	}
}

func TestVerifyCachesWithinTheTTL(t *testing.T) {
	up := newFakeUpstream(t, respondJSON(`{"data":{"id":"1","email":"a@b.c","status":"active"}}`))

	clock := time.Now()
	client := New(Options{
		BaseURL:  up.server.URL,
		CacheTTL: time.Minute,
		Now:      func() time.Time { return clock },
	})

	for range 3 {
		if _, err := client.Verify(context.Background(), "tok"); err != nil {
			t.Fatalf("Verify() 失败: %v", err)
		}
	}
	if got := up.calls.Load(); got != 1 {
		t.Fatalf("TTL 内应只打一次上游, got %d", got)
	}

	// 不同令牌不得共用缓存条目。
	if _, err := client.Verify(context.Background(), "other"); err != nil {
		t.Fatalf("Verify() 失败: %v", err)
	}
	if got := up.calls.Load(); got != 2 {
		t.Fatalf("另一个令牌应重新校验, got %d 次调用", got)
	}

	clock = clock.Add(time.Minute + time.Second)
	if _, err := client.Verify(context.Background(), "tok"); err != nil {
		t.Fatalf("Verify() 失败: %v", err)
	}
	if got := up.calls.Load(); got != 3 {
		t.Fatalf("TTL 过后应重新校验, got %d 次调用", got)
	}
}

func TestVerifyMapsUnauthorizedToInvalidToken(t *testing.T) {
	up := newFakeUpstream(t, func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusUnauthorized)
	})

	client := New(Options{BaseURL: up.server.URL, CacheTTL: time.Minute})
	_, err := client.Verify(context.Background(), "tok")
	if !errors.Is(err, ErrInvalidToken) {
		t.Fatalf("401 应映射为 ErrInvalidToken, got %v", err)
	}

	// 失败结果不进缓存，否则用户在 Sub2API 侧恢复后还要等 TTL 过期。
	_, _ = client.Verify(context.Background(), "tok")
	if got := up.calls.Load(); got != 2 {
		t.Errorf("失败不应被缓存, got %d 次调用", got)
	}
}

// TestVerifyMapsUpstreamFailureToUnavailable 锁住降级策略：
// 上游异常必须显式失败（由调用方渲染 503），绝不能静默放行。
func TestVerifyMapsUpstreamFailureToUnavailable(t *testing.T) {
	cases := []struct {
		name   string
		status int
	}{
		{"上游 500", http.StatusInternalServerError},
		{"上游 502", http.StatusBadGateway},
		{"上游限频", http.StatusTooManyRequests},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			up := newFakeUpstream(t, func(w http.ResponseWriter, _ *http.Request) {
				w.WriteHeader(tc.status)
			})

			_, err := New(Options{BaseURL: up.server.URL}).Verify(context.Background(), "tok")
			if !errors.Is(err, ErrUnavailable) {
				t.Fatalf("err = %v, want ErrUnavailable", err)
			}
			if errors.Is(err, ErrInvalidToken) {
				t.Error("上游异常不得被当成令牌无效，否则用户会被莫名登出")
			}
		})
	}
}

func TestVerifyMapsNetworkFailureToUnavailable(t *testing.T) {
	up := newFakeUpstream(t, respondJSON(`{}`))
	base := up.server.URL
	up.server.Close()

	_, err := New(Options{BaseURL: base}).Verify(context.Background(), "tok")
	if !errors.Is(err, ErrUnavailable) {
		t.Fatalf("上游不可达时 err = %v, want ErrUnavailable", err)
	}
}

func TestVerifyRejectsAnEmptyTokenWithoutCallingUpstream(t *testing.T) {
	up := newFakeUpstream(t, respondJSON(`{"data":{"id":"1"}}`))

	_, err := New(Options{BaseURL: up.server.URL}).Verify(context.Background(), "  ")
	if !errors.Is(err, ErrInvalidToken) {
		t.Fatalf("空令牌 err = %v, want ErrInvalidToken", err)
	}
	if got := up.calls.Load(); got != 0 {
		t.Errorf("空令牌不应打上游, got %d 次调用", got)
	}
}

func TestNewNormalisesTheBaseURL(t *testing.T) {
	if got := New(Options{BaseURL: "https://api.lumio.games/"}).BaseURL(); got != "https://api.lumio.games" {
		t.Errorf("BaseURL() = %q, 末尾斜杠应被去掉", got)
	}
	// 留空时回落到线上默认值，避免部署漏配就把身份校验指向空地址。
	if got := New(Options{}).BaseURL(); got != DefaultBaseURL {
		t.Errorf("BaseURL() = %q, want %q", got, DefaultBaseURL)
	}
}

func TestIdentityActive(t *testing.T) {
	cases := []struct {
		status string
		want   bool
	}{
		{"active", true},
		{"ACTIVE", true},
		{"", true}, // 上游不下发 status 时按可用处理，停用由 Sub2API 直接拒发令牌
		{"disabled", false},
		{"banned", false},
		{"suspended", false},
	}
	for _, tc := range cases {
		if got := (Identity{Status: tc.status}).Active(); got != tc.want {
			t.Errorf("status=%q Active() = %v, want %v", tc.status, got, tc.want)
		}
	}
}
