package domain

import (
	"testing"
	"time"
)

func TestMaskEmail(t *testing.T) {
	cases := []struct{ in, want string }{
		{"wang@gmail.com", "w***g@gmail.com"},
		{"li3@qq.com", "l***3@qq.com"},
		{"chen@163.com", "c***n@163.com"},
		{"ab@example.com", "a***@example.com"},
		{"a@example.com", "a***@example.com"},
		{"not-an-email", "***"},
	}

	for _, tc := range cases {
		if got := MaskEmail(tc.in); got != tc.want {
			t.Errorf("MaskEmail(%q) = %q, want %q", tc.in, got, tc.want)
		}
	}
}

func TestUserDisplayID(t *testing.T) {
	if got := (User{ID: 100986}).DisplayID(); got != "U-100986" {
		t.Errorf("DisplayID() = %q, want U-100986", got)
	}
}

func TestFormatPlatform(t *testing.T) {
	cases := []struct {
		platform, osVersion, arch, want string
	}{
		{"macos", "15", "arm64", "macOS 15 · Apple Silicon"},
		{"macos", "14", "x86_64", "macOS 14 · Intel"},
		{"macos", "13", "", "macOS 13"},
		{"browser", "", "", "浏览器"},
		{"", "", "", ""},
	}

	for _, tc := range cases {
		if got := FormatPlatform(tc.platform, tc.osVersion, tc.arch); got != tc.want {
			t.Errorf("FormatPlatform(%q,%q,%q) = %q, want %q",
				tc.platform, tc.osVersion, tc.arch, got, tc.want)
		}
	}
}

func TestDaysUntil(t *testing.T) {
	now := time.Date(2026, 8, 12, 10, 0, 0, 0, time.UTC)

	cases := []struct {
		name     string
		deadline time.Time
		want     int
	}{
		{"整 30 天", now.AddDate(0, 0, 30), 30},
		{"已过期", now.Add(-time.Hour), 0},
		{"不足一天向上取整", now.Add(12 * time.Hour), 1},
		{"刚好到期", now, 0},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := DaysUntil(now, tc.deadline); got != tc.want {
				t.Errorf("DaysUntil = %d, want %d", got, tc.want)
			}
		})
	}
}

func TestSubscriptionSnapshot(t *testing.T) {
	now := time.Date(2026, 8, 12, 10, 0, 0, 0, time.UTC)
	trial, paid := KindTrial, KindPaid

	t.Run("未订阅", func(t *testing.T) {
		got := Subscription{}.Snapshot(now)
		if got.Status != EntitlementNone {
			t.Errorf("状态 = %q, want none", got.Status)
		}
	})

	t.Run("试用中", func(t *testing.T) {
		expires := now.AddDate(0, 0, 23)
		got := Subscription{Kind: &trial, ExpiresAt: &expires}.Snapshot(now)
		if got.Status != EntitlementTrialing {
			t.Errorf("状态 = %q, want trialing", got.Status)
		}
		if got.DaysLeft != 23 {
			t.Errorf("剩余天数 = %d, want 23", got.DaysLeft)
		}
		if got.ExpiringSoon {
			t.Error("剩余 23 天不应触发到期提醒")
		}
	})

	t.Run("已订阅且即将到期", func(t *testing.T) {
		expires := now.AddDate(0, 0, 3)
		got := Subscription{Kind: &paid, ExpiresAt: &expires}.Snapshot(now)
		if got.Status != EntitlementActive {
			t.Errorf("状态 = %q, want active", got.Status)
		}
		// 剩余 ≤3 天要触发 APP 顶部横幅与橙色文字。
		if !got.ExpiringSoon {
			t.Error("剩余 3 天应触发到期提醒")
		}
	})

	t.Run("已过期", func(t *testing.T) {
		expires := now.AddDate(0, 0, -1)
		got := Subscription{Kind: &paid, ExpiresAt: &expires}.Snapshot(now)
		if got.Status != EntitlementExpired {
			t.Errorf("状态 = %q, want expired", got.Status)
		}
		if got.DaysLeft != 0 {
			t.Errorf("过期后剩余天数应为 0, got %d", got.DaysLeft)
		}
	})
}

func TestNormalizeEmail(t *testing.T) {
	if got := NormalizeEmail("  Mary@Example.COM "); got != "mary@example.com" {
		t.Errorf("NormalizeEmail = %q", got)
	}
}
