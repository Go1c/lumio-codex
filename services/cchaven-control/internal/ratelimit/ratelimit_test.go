package ratelimit

import (
	"sync"
	"testing"
	"time"
)

func TestAllowWithinLimit(t *testing.T) {
	limiter := New()
	rule := Rule{Limit: 3, Window: time.Minute}

	for i := range 3 {
		if ok, _ := limiter.Allow("key", rule); !ok {
			t.Fatalf("第 %d 次调用应放行", i+1)
		}
	}

	ok, retry := limiter.Allow("key", rule)
	if ok {
		t.Error("超出配额应被拒绝")
	}
	if retry <= 0 || retry > time.Minute {
		t.Errorf("重试等待时间不合理: %v", retry)
	}
}

func TestWindowRollover(t *testing.T) {
	now := time.Now()
	limiter := NewWithClock(func() time.Time { return now })
	rule := Rule{Limit: 1, Window: time.Minute}

	if ok, _ := limiter.Allow("key", rule); !ok {
		t.Fatal("首次调用应放行")
	}
	if ok, _ := limiter.Allow("key", rule); ok {
		t.Fatal("同一窗口内第二次应被拒绝")
	}

	now = now.Add(time.Minute + time.Second)
	if ok, _ := limiter.Allow("key", rule); !ok {
		t.Error("窗口滚动后应恢复配额")
	}
}

func TestKeysAreIndependent(t *testing.T) {
	limiter := New()
	rule := Rule{Limit: 1, Window: time.Minute}

	limiter.Allow("ip:1.1.1.1", rule)
	if ok, _ := limiter.Allow("ip:2.2.2.2", rule); !ok {
		t.Error("不同键的配额应互不影响")
	}
}

func TestReset(t *testing.T) {
	limiter := New()
	rule := Rule{Limit: 1, Window: time.Minute}

	limiter.Allow("key", rule)
	limiter.Reset("key")

	if ok, _ := limiter.Allow("key", rule); !ok {
		t.Error("Reset 后应恢复配额")
	}
}

// TestConcurrentAllowIsSafe 在竞态检测下验证并发安全，并确认配额不会被超发。
func TestConcurrentAllowIsSafe(t *testing.T) {
	limiter := New()
	rule := Rule{Limit: 50, Window: time.Minute}

	var wg sync.WaitGroup
	var mu sync.Mutex
	allowed := 0

	for range 200 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if ok, _ := limiter.Allow("shared", rule); ok {
				mu.Lock()
				allowed++
				mu.Unlock()
			}
		}()
	}
	wg.Wait()

	if allowed != 50 {
		t.Errorf("放行次数 = %d, want 50", allowed)
	}
}
