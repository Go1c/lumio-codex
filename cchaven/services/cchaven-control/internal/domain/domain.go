// Package domain 定义控制面的核心实体与派生规则，不依赖数据库与 HTTP。
package domain

import (
	"fmt"
	"math"
	"strings"
	"time"

	"github.com/google/uuid"
)

// —— 用户 ——

// UserStatus 是账号状态。
//
// 注意：登录失败导致的「锁定」不是账号状态，而是 User.LockedUntil 表示的临时态
// （15 分钟自动解除）。把它建模为状态会与后台「已禁用」筛选混淆。
type UserStatus string

const (
	// UserPendingEmail 已注册但邮箱未验证，不得发放任何会话。
	UserPendingEmail UserStatus = "pending_email"
	// UserActive 正常账号。
	UserActive UserStatus = "active"
	// UserDisabled 被管理员停用。
	UserDisabled UserStatus = "disabled"
)

// RegistrationSource 是注册来源，对应后台用户列表「来源」列。
type RegistrationSource string

const (
	// SourceOrganic 自然流量。
	SourceOrganic RegistrationSource = "organic"
	// SourceInvite 好友邀请。
	SourceInvite RegistrationSource = "invite"
	// SourceOther 其他渠道（带 utm_source 且非邀请）。
	SourceOther RegistrationSource = "other"
)

// User 是账号实体。
type User struct {
	ID                  int64
	Email               string
	PasswordHash        string
	DisplayName         string
	Status              UserStatus
	EmailVerifiedAt     *time.Time
	LockedUntil         *time.Time
	FailedLoginCount    int
	RegistrationSource  RegistrationSource
	ReferredByUserID    *int64
	TrialGrantedAt      *time.Time
	DeletionRequestedAt *time.Time
	DisabledAt          *time.Time
	DisabledReason      string
	LastActiveAt        *time.Time
	CreatedAt           time.Time
	UpdatedAt           time.Time
}

// DisplayID 返回对外展示的注册号，形如 U-100986。
func (u User) DisplayID() string { return fmt.Sprintf("U-%d", u.ID) }

// IsLocked 报告账号此刻是否处于登录锁定期。
func (u User) IsLocked(now time.Time) bool {
	return u.LockedUntil != nil && u.LockedUntil.After(now)
}

// LockRemaining 返回锁定剩余时长。
func (u User) LockRemaining(now time.Time) time.Duration {
	if !u.IsLocked(now) {
		return 0
	}
	return u.LockedUntil.Sub(now)
}

// —— 订阅 ——

// EntitlementStatus 是对外的订阅状态，由 kind 与 expires_at 派生，不落库。
type EntitlementStatus string

const (
	// EntitlementNone 未订阅。
	EntitlementNone EntitlementStatus = "none"
	// EntitlementTrialing 免费试用中。
	EntitlementTrialing EntitlementStatus = "trialing"
	// EntitlementActive 已订阅。
	EntitlementActive EntitlementStatus = "active"
	// EntitlementExpired 已过期。
	EntitlementExpired EntitlementStatus = "expired"
)

// SubscriptionKind 区分试用与付费。
type SubscriptionKind string

const (
	// KindTrial 免费试用。
	KindTrial SubscriptionKind = "trial"
	// KindPaid 付费订阅。
	KindPaid SubscriptionKind = "paid"
)

// Subscription 是订阅记录，每个用户恒有一行。
type Subscription struct {
	UserID         int64
	Kind           *SubscriptionKind
	ExpiresAt      *time.Time
	TrialExpiresAt *time.Time
	BonusDaysTotal int
	UpdatedAt      time.Time
}

// Entitlement 是给前端的订阅快照，驱动官网徽标与 APP 账户菜单。
type Entitlement struct {
	Status         EntitlementStatus `json:"status"`
	Kind           string            `json:"kind,omitempty"`
	ExpiresAt      *time.Time        `json:"expires_at,omitempty"`
	DaysLeft       int               `json:"days_left"`
	BonusDaysTotal int               `json:"bonus_days_total"`
	// ExpiringSoon 对应「剩余 ≤3 天转橙色 + 顶部横幅」的提醒阈值。
	ExpiringSoon bool `json:"expiring_soon"`
}

// Snapshot 把订阅记录换算为前端可直接渲染的快照。
func (s Subscription) Snapshot(now time.Time) Entitlement {
	e := Entitlement{Status: EntitlementNone, BonusDaysTotal: s.BonusDaysTotal}
	if s.Kind != nil {
		e.Kind = string(*s.Kind)
	}
	if s.ExpiresAt == nil {
		return e
	}

	e.ExpiresAt = s.ExpiresAt
	if !s.ExpiresAt.After(now) {
		e.Status = EntitlementExpired
		return e
	}

	e.DaysLeft = DaysUntil(now, *s.ExpiresAt)
	e.ExpiringSoon = e.DaysLeft <= 3
	if s.Kind != nil && *s.Kind == KindTrial {
		e.Status = EntitlementTrialing
	} else {
		e.Status = EntitlementActive
	}
	return e
}

// DaysUntil 返回剩余天数，向上取整——剩 0.5 天对用户来说仍是「剩余 1 天」。
func DaysUntil(now, deadline time.Time) int {
	if !deadline.After(now) {
		return 0
	}
	return int(math.Ceil(deadline.Sub(now).Hours() / 24))
}

// —— 会话 ——

// SessionClient 区分官网浏览器会话与桌面 APP 会话。
type SessionClient string

const (
	// ClientWeb 官网浏览器会话。
	ClientWeb SessionClient = "web"
	// ClientApp 桌面 APP 会话（经 OAuth 授权取得）。
	ClientApp SessionClient = "app"
)

// 会话族撤销原因。
const (
	RevokeUserLogout     = "user_logout"
	RevokeUserRevoke     = "user_revoke"
	RevokeOthers         = "revoke_others"
	RevokePasswordChange = "password_change"
	RevokePasswordReset  = "password_reset"
	RevokeAdminDisable   = "admin_disable"
	RevokeReuseDetected  = "reuse_detected"
	RevokeAccountDeleted = "account_deleted"
)

// SessionFamily 是一次登录，对应官网「登录设备与授权」列表中的一行。
type SessionFamily struct {
	ID          uuid.UUID
	UserID      int64
	Client      SessionClient
	DeviceName  string
	Platform    string
	OSVersion   string
	Arch        string
	AppVersion  string
	UserAgent   string
	IP          string
	IPRegion    string
	CreatedAt   time.Time
	LastSeenAt  time.Time
	RevokedAt   *time.Time
	OAuthClient string
}

// PlatformDetail 拼出后台「使用平台」列所需的展示串，如「macOS 15 · Apple Silicon」。
func (s SessionFamily) PlatformDetail() string {
	return FormatPlatform(s.Platform, s.OSVersion, s.Arch)
}

// FormatPlatform 把平台三元组格式化为展示串。数据不全时尽量降级而不是留空。
func FormatPlatform(platform, osVersion, arch string) string {
	if platform == "" && osVersion == "" {
		return ""
	}

	name := "macOS"
	if platform == "browser" {
		name = "浏览器"
	}
	if osVersion != "" {
		name += " " + osVersion
	}

	switch arch {
	case "arm64":
		return name + " · Apple Silicon"
	case "x86_64", "amd64":
		return name + " · Intel"
	default:
		return name
	}
}

// —— 邀请 ——

// ReferralStage 是三步闭环的进度：注册 → 首次登录 APP。
type ReferralStage string

const (
	// StageRegistered 已注册，尚未登录 APP。
	StageRegistered ReferralStage = "registered"
	// StageActivated 已注册并首次登录 APP，闭环完成。
	StageActivated ReferralStage = "activated"
)

// ReferralAttribution 是一条邀请归因记录。
type ReferralAttribution struct {
	ID                    int64
	Code                  string
	InviterUserID         int64
	InviteeUserID         int64
	Stage                 ReferralStage
	RegisteredAt          time.Time
	ActivatedAt           *time.Time
	TrialGranted          bool
	InviterBonusDays      int
	InviterBonusGrantedAt *time.Time
}

// —— 订单 ——

// OrderStatus 是订单状态，与后台筛选 chips 一一对应。
type OrderStatus string

const (
	OrderPending   OrderStatus = "pending"   // 待支付
	OrderPaid      OrderStatus = "paid"      // 已支付
	OrderRefunding OrderStatus = "refunding" // 退款中
	OrderRefunded  OrderStatus = "refunded"  // 已退款
	OrderFailed    OrderStatus = "failed"    // 支付失败
)

// PaymentChannel 是支付渠道。
type PaymentChannel string

const (
	ChannelAlipay PaymentChannel = "alipay" // 支付宝
	ChannelWechat PaymentChannel = "wechat" // 微信支付
	ChannelCard   PaymentChannel = "card"   // 银行卡
	ChannelMock   PaymentChannel = "mock"   // M1 mock 通道
)

// Order 是一笔订单。
type Order struct {
	ID            int64
	OrderNo       string
	UserID        int64
	UserEmail     string
	AmountCents   int64
	Currency      string
	Channel       PaymentChannel
	Status        OrderStatus
	PeriodMonths  int
	Provider      string
	ProviderTxnID *string
	PaidAt        *time.Time
	CreatedAt     time.Time
}

// —— 展示辅助 ——

// MaskEmail 对邮箱打码，用于后台列表与邀请进度列表。
//
// 规则：本地部分保留首尾各一个字符，中间固定三个星号（w***g@gmail.com）；
// 本地部分不足 3 个字符时只保留首字符。原型 mock 数据里存在 chen***@ 与 w***g@
// 两种写法，此处统一取信息泄露更少的一种。
func MaskEmail(email string) string {
	local, domain, ok := strings.Cut(email, "@")
	if !ok || local == "" {
		return "***"
	}

	runes := []rune(local)
	switch {
	case len(runes) < 3:
		return string(runes[0]) + "***@" + domain
	default:
		return string(runes[0]) + "***" + string(runes[len(runes)-1]) + "@" + domain
	}
}

// NormalizeEmail 统一邮箱大小写与两端空白，作为唯一性判断的依据。
func NormalizeEmail(email string) string {
	return strings.ToLower(strings.TrimSpace(email))
}
