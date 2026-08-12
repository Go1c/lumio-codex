// Package ratelimit 提供进程内固定窗口限频。
//
// M1 单实例部署足够；多实例时把 Limiter 换成 Redis 实现即可，
// 调用方只依赖 Allow 这一个方法。
package ratelimit

import (
	"sync"
	"time"
)

// Rule 描述一条限频规则：window 时间窗内最多 limit 次。
type Rule struct {
	Limit  int
	Window time.Duration
}

type counter struct {
	count       int
	windowStart time.Time
}

// Limiter 是并发安全的固定窗口计数器。
type Limiter struct {
	mu       sync.Mutex
	counters map[string]*counter
	now      func() time.Time
	lastGC   time.Time
}

// New 构造限频器。
func New() *Limiter { return NewWithClock(time.Now) }

// NewWithClock 构造使用自定义时钟的限频器，便于测试。
func NewWithClock(now func() time.Time) *Limiter {
	return &Limiter{counters: map[string]*counter{}, now: now, lastGC: now()}
}

// Allow 消费一次配额。返回是否放行，以及被拒时距窗口结束的剩余时间。
func (l *Limiter) Allow(key string, rule Rule) (bool, time.Duration) {
	l.mu.Lock()
	defer l.mu.Unlock()

	now := l.now()
	l.gcLocked(now)

	c, ok := l.counters[key]
	if !ok || now.Sub(c.windowStart) >= rule.Window {
		l.counters[key] = &counter{count: 1, windowStart: now}
		return true, 0
	}

	if c.count >= rule.Limit {
		return false, rule.Window - now.Sub(c.windowStart)
	}
	c.count++
	return true, 0
}

// Reset 清除某个键的计数，用于登录成功后立即恢复配额。
func (l *Limiter) Reset(key string) {
	l.mu.Lock()
	defer l.mu.Unlock()
	delete(l.counters, key)
}

// gcLocked 每分钟清理一次过期计数，避免键无限增长。
func (l *Limiter) gcLocked(now time.Time) {
	if now.Sub(l.lastGC) < time.Minute {
		return
	}
	l.lastGC = now

	for key, c := range l.counters {
		if now.Sub(c.windowStart) > time.Hour {
			delete(l.counters, key)
		}
	}
}
