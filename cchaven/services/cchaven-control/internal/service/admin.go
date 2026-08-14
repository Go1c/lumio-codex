package service

import (
	"context"
	"errors"
	"log/slog"
	"maps"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/pquerna/otp/totp"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/apperr"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/db"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/domain"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/payments"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/security"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/store"
)

const totpIssuer = "CCHaven Admin"

// 管理员角色。权限矩阵见下方 roleCapabilities。
const (
	// RoleOwner 超级管理员。
	RoleOwner = "owner"
	// RoleOps 运营。
	RoleOps = "ops"
	// RoleSupport 客服，全线只读：能看列表与指标，不能做任何写操作或读明文邮箱。
	RoleSupport = "support"
)

// AdminCapability 是一项受角色控制的管理端能力。
type AdminCapability string

const (
	// CapViewUserDetail 查看用户详情，其中含明文邮箱。
	CapViewUserDetail AdminCapability = "view_user_detail"
	// CapManageUsers 禁用与解禁用户。
	CapManageUsers AdminCapability = "manage_users"
	// CapRefundOrder 对订单发起退款。
	CapRefundOrder AdminCapability = "refund_order"
	// CapEditOpsConfig 修改运营配置。
	CapEditOpsConfig AdminCapability = "edit_ops_config"
	// CapExportOrders 导出订单 CSV。
	CapExportOrders AdminCapability = "export_orders"
)

// elevatedCapabilities 是 owner 与 ops 当前共有的全部能力。
//
// owner 与 ops 不做区分是刻意的：两者眼下没有任何真实职责差异，硬造一个只会变成
// 需要靠记忆维持的规则。保留 owner 这个角色是为了将来的管理员账号管理
// （新增/停用管理员、修改他人角色）只给 owner——那是第一个值得区分的能力，
// 届时它单独进 roleCapabilities[RoleOwner]，而不是加进这个共享集合。
var elevatedCapabilities = []AdminCapability{
	CapViewUserDetail,
	CapManageUsers,
	CapRefundOrder,
	CapEditOpsConfig,
	CapExportOrders,
}

// roleCapabilities 是权限矩阵的唯一出处，默认拒绝：表里没有登记的组合一律无权。
//
// 这里选择「集中的能力表 + 语义化谓词」，而不是在各 handler 里散写 `if role == ...`，
// 也不是只留一个 IsReadOnly(role)：
//   - 散写是这套权限倒挂的成因。矩阵散在各处时，没有任何一处能一眼看全，
//     新增接口时也没有东西提醒你补一格，于是「support 不能看邮箱却能禁用用户」这种
//     破坏性操作门槛低于读取的组合能长期存在。
//   - 单一 IsReadOnly 判定确实更省事，但它把「这个接口算不算写操作」变成每个调用点的
//     口头约定；而且一旦 owner 与 ops 出现差异（管理员账号管理迟早会），要全量返工。
//   - 能力表的代价是新增能力得改两处（加常量、加进集合），换来的是矩阵可被一次读完、
//     可被单测逐格锁定（admin_capability_test.go），调用点读起来也是业务语义而不是角色字符串。
//
// 新增受控接口时的固定动作：加一个 Cap*、把它列进对应角色、在 service 方法入口调谓词、
// 拒绝路径走 auditDenied 写 `{action}_denied`。
var roleCapabilities = map[string]map[AdminCapability]bool{
	RoleOwner: capabilitySet(elevatedCapabilities...),
	RoleOps:   capabilitySet(elevatedCapabilities...),
	// support 全线只读，一格都不给：破坏性操作的权限门槛不得低于读取敏感信息。
	RoleSupport: capabilitySet(),
}

func capabilitySet(caps ...AdminCapability) map[AdminCapability]bool {
	set := make(map[AdminCapability]bool, len(caps))
	for _, c := range caps {
		set[c] = true
	}
	return set
}

// Can 报告某个角色是否具备某项能力。未知角色、未登记的能力一律拒绝。
func Can(role string, capability AdminCapability) bool {
	return roleCapabilities[role][capability]
}

// CanViewUserDetail 报告该角色是否可以查看用户明文邮箱。
func CanViewUserDetail(role string) bool { return Can(role, CapViewUserDetail) }

// CanManageUsers 报告该角色是否可以禁用/解禁用户。
func CanManageUsers(role string) bool { return Can(role, CapManageUsers) }

// CanRefundOrder 报告该角色是否可以发起退款。
func CanRefundOrder(role string) bool { return Can(role, CapRefundOrder) }

// CanEditOpsConfig 报告该角色是否可以修改运营配置。
func CanEditOpsConfig(role string) bool { return Can(role, CapEditOpsConfig) }

// CanExportOrders 报告该角色是否可以导出订单 CSV。
func CanExportOrders(role string) bool { return Can(role, CapExportOrders) }

// AdminLoginResult 是管理员登录结果。
//
// 已启用两步验证时只发放「半会话」（mfa_passed=false），它只能用于提交 TOTP 码，
// 访问任何业务接口都会被拒绝。
type AdminLoginResult struct {
	Token       string `json:"-"`
	MFARequired bool   `json:"mfa_required"`
	MFAEnrolled bool   `json:"mfa_enrolled"`
}

// AdminPrincipal 是通过鉴权的管理员身份。
type AdminPrincipal struct {
	Admin     store.Admin
	SessionID uuid.UUID
}

// AdminLogin 校验管理员口令。
func (s *Service) AdminLogin(ctx context.Context, email, password, ip, userAgent string) (AdminLoginResult, error) {
	now := s.now()

	admin, err := store.GetAdminByEmail(ctx, s.Pool, email)
	if err != nil {
		if errors.Is(err, store.ErrNotFound) {
			_ = s.Hasher.Verify(password, dummyPasswordHash)
			return AdminLoginResult{}, apperr.InvalidCredentials()
		}
		return AdminLoginResult{}, err
	}
	if admin.LockedUntil != nil && admin.LockedUntil.After(now) {
		return AdminLoginResult{}, apperr.AccountLocked(admin.LockedUntil.Sub(now))
	}
	if admin.Status != "active" {
		return AdminLoginResult{}, apperr.AccountDisabled()
	}

	if err := s.Hasher.Verify(password, admin.PasswordHash); err != nil {
		if !errors.Is(err, security.ErrPasswordMismatch) {
			return AdminLoginResult{}, err
		}
		if err := store.RecordAdminLoginFailure(ctx, s.Pool, admin.ID,
			AdminLoginFailureThreshold, AdminLoginLockDuration, now); err != nil {
			return AdminLoginResult{}, err
		}
		return AdminLoginResult{}, apperr.InvalidCredentials()
	}

	if err := store.ClearAdminLoginFailures(ctx, s.Pool, admin.ID, now); err != nil {
		return AdminLoginResult{}, err
	}

	token, err := security.RandomToken(32)
	if err != nil {
		return AdminLoginResult{}, err
	}
	mfaPassed := !admin.TOTPEnabled()
	if _, err := store.CreateAdminSession(ctx, s.Pool, admin.ID, security.HashToken(token),
		mfaPassed, ip, userAgent, now.Add(s.Cfg.AdminSessionTTL)); err != nil {
		return AdminLoginResult{}, err
	}

	return AdminLoginResult{
		Token:       token,
		MFARequired: !mfaPassed,
		MFAEnrolled: admin.TOTPEnabled(),
	}, nil
}

// AdminVerifyTOTP 校验两步验证码并升级会话。
//
// 错误码必须与口令登录一样计入按账号的失败锁定（QA S-1）：TOTP 只有 10^6 种
// 取值，拿到口令的攻击者若能无限重试，小时级在线穷举即可命中。复用口令的
// 5 次锁 15 分钟机制，与路由层的按 IP 限频互为补充。
func (s *Service) AdminVerifyTOTP(ctx context.Context, token, code string) error {
	session, err := store.GetAdminSessionByHash(ctx, s.Pool, security.HashToken(token), s.now())
	if err != nil {
		if errors.Is(err, store.ErrNotFound) {
			return apperr.Unauthorized()
		}
		return err
	}
	if session.MFAPassed {
		return nil
	}

	admin, err := store.GetAdminByID(ctx, s.Pool, session.AdminID)
	if err != nil {
		return err
	}
	if admin.LockedUntil != nil && admin.LockedUntil.After(s.now()) {
		return apperr.AccountLocked(admin.LockedUntil.Sub(s.now()))
	}
	if !admin.TOTPEnabled() {
		return apperr.AdminMFAInvalid()
	}

	secret, err := s.Cipher.Decrypt(*admin.TOTPSecretEnc)
	if err != nil {
		return err
	}
	if !totp.Validate(code, secret) {
		if err := store.RecordAdminLoginFailure(ctx, s.Pool, admin.ID,
			AdminLoginFailureThreshold, AdminLoginLockDuration, s.now()); err != nil {
			return err
		}
		return apperr.AdminMFAInvalid()
	}
	if err := store.ClearAdminLoginFailures(ctx, s.Pool, admin.ID, s.now()); err != nil {
		return err
	}
	return store.MarkAdminSessionMFAPassed(ctx, s.Pool, session.ID)
}

// AuthenticateAdmin 校验管理端会话。未通过两步验证的半会话一律拒绝。
func (s *Service) AuthenticateAdmin(ctx context.Context, token string) (AdminPrincipal, error) {
	if token == "" {
		return AdminPrincipal{}, apperr.Unauthorized()
	}

	session, err := store.GetAdminSessionByHash(ctx, s.Pool, security.HashToken(token), s.now())
	if err != nil {
		if errors.Is(err, store.ErrNotFound) {
			return AdminPrincipal{}, apperr.Unauthorized()
		}
		return AdminPrincipal{}, err
	}
	if !session.MFAPassed {
		return AdminPrincipal{}, apperr.AdminMFARequired()
	}

	admin, err := store.GetAdminByID(ctx, s.Pool, session.AdminID)
	if err != nil {
		return AdminPrincipal{}, err
	}
	if admin.Status != "active" {
		return AdminPrincipal{}, apperr.AccountDisabled()
	}
	return AdminPrincipal{Admin: admin, SessionID: session.ID}, nil
}

// AdminLogout 撤销管理端会话。
func (s *Service) AdminLogout(ctx context.Context, sessionID uuid.UUID) error {
	return store.RevokeAdminSession(ctx, s.Pool, sessionID, s.now())
}

// TOTPEnrollment 是两步验证注册信息。
type TOTPEnrollment struct {
	Secret string `json:"secret"`
	URI    string `json:"uri"`
}

// AdminSetupTOTP 生成 TOTP 种子。种子加密后暂存，待用户提交正确验证码才正式启用。
func (s *Service) AdminSetupTOTP(ctx context.Context, adminID int64) (TOTPEnrollment, error) {
	admin, err := store.GetAdminByID(ctx, s.Pool, adminID)
	if err != nil {
		return TOTPEnrollment{}, err
	}

	key, err := totp.Generate(totp.GenerateOpts{Issuer: totpIssuer, AccountName: admin.Email})
	if err != nil {
		return TOTPEnrollment{}, err
	}

	encrypted, err := s.Cipher.Encrypt(key.Secret())
	if err != nil {
		return TOTPEnrollment{}, err
	}
	// enabledAt 传 nil：种子已存但尚未生效，此时登录仍不要求两步验证。
	if err := store.SetAdminTOTP(ctx, s.Pool, adminID, encrypted, nil); err != nil {
		return TOTPEnrollment{}, err
	}

	return TOTPEnrollment{Secret: key.Secret(), URI: key.URL()}, nil
}

// AdminEnableTOTP 校验首个验证码并正式启用两步验证。
func (s *Service) AdminEnableTOTP(ctx context.Context, adminID int64, code string) error {
	admin, err := store.GetAdminByID(ctx, s.Pool, adminID)
	if err != nil {
		return err
	}
	if admin.TOTPSecretEnc == nil {
		return apperr.InvalidParams()
	}

	secret, err := s.Cipher.Decrypt(*admin.TOTPSecretEnc)
	if err != nil {
		return err
	}
	if !totp.Validate(code, secret) {
		return apperr.AdminMFAInvalid()
	}

	now := s.now()
	return store.SetAdminTOTP(ctx, s.Pool, adminID, *admin.TOTPSecretEnc, &now)
}

// —— 指标 ——

// MetricCard 是仪表盘上的一张指标卡。
//
// Value 为 nil 表示缺数，前端显示「—」而不是 0，避免把「没数据」误读成「为零」。
type MetricCard struct {
	Value      *float64 `json:"value"`
	Delta      *float64 `json:"delta,omitempty"`
	Secondary  *float64 `json:"secondary,omitempty"`
	SecondaryB *float64 `json:"secondary_b,omitempty"`
}

// MetricsOverview 是仪表盘六张卡的数据。
type MetricsOverview struct {
	DAU              MetricCard `json:"dau"`
	Signups          MetricCard `json:"signups"`
	Subscribers      MetricCard `json:"subscribers"`
	Revenue          MetricCard `json:"revenue"`
	TrialConversion  MetricCard `json:"trial_conversion"`
	RetentionD7      MetricCard `json:"retention_d7"`
	GeneratedAtLabel string     `json:"generated_at"`
}

// MetricsOverview 汇总仪表盘核心指标。
func (s *Service) MetricsOverview(ctx context.Context) (MetricsOverview, error) {
	now := s.now()
	today := now.Truncate(24 * time.Hour)
	yesterday := today.AddDate(0, 0, -1)

	var out MetricsOverview

	todayDAU, err := store.CountActiveUsers(ctx, s.Pool, today)
	if err != nil {
		return MetricsOverview{}, err
	}
	yesterdayDAU, err := store.CountActiveUsers(ctx, s.Pool, yesterday)
	if err != nil {
		return MetricsOverview{}, err
	}
	out.DAU = MetricCard{Value: floatp(float64(todayDAU))}
	if yesterdayDAU > 0 {
		out.DAU.Delta = floatp((float64(todayDAU) - float64(yesterdayDAU)) / float64(yesterdayDAU))
	}

	signups, invited, err := store.SignupCounts(ctx, s.Pool, today, today.AddDate(0, 0, 1))
	if err != nil {
		return MetricsOverview{}, err
	}
	out.Signups = MetricCard{Value: floatp(float64(signups)), Secondary: floatp(float64(invited))}

	paid, trialing, err := store.SubscriberCounts(ctx, s.Pool, now)
	if err != nil {
		return MetricsOverview{}, err
	}
	out.Subscribers = MetricCard{Value: floatp(float64(paid)), Secondary: floatp(float64(trialing))}

	orderCount, amount, err := store.TodayOrderSummary(ctx, s.Pool, today, today.AddDate(0, 0, 1))
	if err != nil {
		return MetricsOverview{}, err
	}
	out.Revenue = MetricCard{Value: floatp(float64(amount)), Secondary: floatp(float64(orderCount))}

	if rate, ok, err := store.TrialConversionRate(ctx, s.Pool, now, 30); err != nil {
		return MetricsOverview{}, err
	} else if ok {
		out.TrialConversion = MetricCard{Value: floatp(rate)}
	}

	if rate, ok, err := store.RetentionD7(ctx, s.Pool, today); err != nil {
		return MetricsOverview{}, err
	} else if ok {
		out.RetentionD7 = MetricCard{Value: floatp(rate)}
		if prev, prevOK, err := store.RetentionD7(ctx, s.Pool, today.AddDate(0, 0, -7)); err != nil {
			return MetricsOverview{}, err
		} else if prevOK {
			out.RetentionD7.Delta = floatp(rate - prev)
		}
	}

	out.GeneratedAtLabel = now.Format(time.RFC3339)
	return out, nil
}

// Distributions 是三组分布图数据。
type Distributions struct {
	Platform   []store.Bucket `json:"platform"`
	AppVersion []store.Bucket `json:"app_version"`
	Source     []store.Bucket `json:"source"`
}

// Distributions 汇总平台、APP 版本与注册来源分布。
func (s *Service) Distributions(ctx context.Context, days int) (Distributions, error) {
	now := s.now()

	platform, err := store.PlatformDistribution(ctx, s.Pool, now, days)
	if err != nil {
		return Distributions{}, err
	}
	versions, err := store.AppVersionDistribution(ctx, s.Pool, now, days)
	if err != nil {
		return Distributions{}, err
	}
	sources, err := store.SourceDistribution(ctx, s.Pool, now, days)
	if err != nil {
		return Distributions{}, err
	}
	return Distributions{Platform: platform, AppVersion: versions, Source: sources}, nil
}

// DailyActive 返回近 days 天的日活序列。
func (s *Service) DailyActive(ctx context.Context, days int) ([]store.DailyCount, error) {
	return store.DailyActiveSeries(ctx, s.Pool, s.now().Truncate(24*time.Hour), days)
}

// —— 用户管理 ——

// AdminUserView 是后台用户列表的一行。邮箱默认打码，详情接口才返回明文。
//
// ID 是展示用的注册号（`U-100986`），UserID 是调用详情、禁用、退款等接口用的主键。
// 两者并存是刻意的：前端不需要、也不应该从展示号反解出主键。
type AdminUserView struct {
	ID           string     `json:"id"`
	UserID       int64      `json:"user_id"`
	EmailMasked  string     `json:"email_masked"`
	CreatedAt    time.Time  `json:"created_at"`
	Source       string     `json:"source"`
	InviterID    string     `json:"inviter_id,omitempty"`
	Platform     string     `json:"platform"`
	SubState     string     `json:"sub_state"`
	LastActiveAt *time.Time `json:"last_active_at,omitempty"`
}

// ListUsers 分页查询后台用户列表。
func (s *Service) ListUsers(
	ctx context.Context, query, status string, page, pageSize int,
) ([]AdminUserView, int64, error) {
	rows, total, err := store.ListAdminUsers(
		ctx, s.Pool, query, status, pageSize, (page-1)*pageSize, s.now())
	if err != nil {
		return nil, 0, err
	}

	out := make([]AdminUserView, 0, len(rows))
	for _, r := range rows {
		view := AdminUserView{
			ID:           "U-" + strconv.FormatInt(r.ID, 10),
			UserID:       r.ID,
			EmailMasked:  domain.MaskEmail(r.Email),
			CreatedAt:    r.CreatedAt,
			Source:       sourceLabel(r.Source),
			Platform:     r.Platform,
			SubState:     r.SubState,
			LastActiveAt: r.LastActiveAt,
		}
		if r.InviterID != nil {
			view.InviterID = "U-" + strconv.FormatInt(*r.InviterID, 10)
		}
		out = append(out, view)
	}
	return out, total, nil
}

// AdminUserProfile 是用户详情页头部的账号信息。邮箱在此为明文，故整个详情受二次权限保护。
//
// 与列表行一致：ID 是展示用注册号，UserID 是调接口用的主键。
type AdminUserProfile struct {
	ID                  string     `json:"id"`
	UserID              int64      `json:"user_id"`
	Email               string     `json:"email"`
	DisplayName         string     `json:"display_name"`
	Status              string     `json:"status"`
	CreatedAt           time.Time  `json:"created_at"`
	Source              string     `json:"source"`
	InviterID           string     `json:"inviter_id,omitempty"`
	LastActiveAt        *time.Time `json:"last_active_at,omitempty"`
	DeletionRequestedAt *time.Time `json:"deletion_requested_at,omitempty"`
}

// AdminDeviceView 是用户详情页设备列表中的一台设备。
type AdminDeviceView struct {
	DeviceID    string    `json:"device_id"`
	Platform    string    `json:"platform"`
	AppVersion  string    `json:"app_version,omitempty"`
	FirstSeenAt time.Time `json:"first_seen_at"`
	LastSeenAt  time.Time `json:"last_seen_at"`
}

// AdminReferralView 是用户详情页的邀请汇总与进度。
type AdminReferralView struct {
	InvitedCount   int            `json:"invited_count"`
	TotalBonusDays int            `json:"total_bonus_days"`
	Items          []ReferralItem `json:"items"`
}

// AdminUserDetail 是后台用户详情页的全部数据。
type AdminUserDetail struct {
	User        AdminUserProfile   `json:"user"`
	Entitlement domain.Entitlement `json:"entitlement"`
	Devices     []AdminDeviceView  `json:"devices"`
	Referral    AdminReferralView  `json:"referral"`
	Orders      []AdminOrderView   `json:"orders"`
}

// AdminUserDetailOrderLimit 是详情页「最近订单」的条数上限，更早的订单去订单页查。
const AdminUserDetailOrderLimit = 10

// UserDetail 读取后台用户详情，并为每次访问留下审计记录。
//
// 查看明文邮箱本身就是需要留痕的敏感操作，因此审计与读取放在同一个事务里：
// 审计写不进去就不返回数据，不存在「看过但查不到记录」的窗口。
func (s *Service) UserDetail(
	ctx context.Context, actor AdminPrincipal, userID int64, ip, userAgent string,
) (AdminUserDetail, error) {
	if !CanViewUserDetail(actor.Admin.Role) {
		s.auditDenied(ctx, actor, "user.view_detail", "user",
			strconv.FormatInt(userID, 10), ip, userAgent)
		return AdminUserDetail{}, apperr.Forbidden()
	}

	now := s.now()
	var detail AdminUserDetail

	err := db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		user, err := store.GetUserByID(ctx, tx, userID)
		if err != nil {
			if errors.Is(err, store.ErrNotFound) {
				return apperr.NotFound()
			}
			return err
		}

		profile := AdminUserProfile{
			ID:                  user.DisplayID(),
			UserID:              user.ID,
			Email:               user.Email,
			DisplayName:         user.DisplayName,
			Status:              string(user.Status),
			CreatedAt:           user.CreatedAt,
			Source:              sourceLabel(string(user.RegistrationSource)),
			LastActiveAt:        user.LastActiveAt,
			DeletionRequestedAt: user.DeletionRequestedAt,
		}
		if user.ReferredByUserID != nil {
			profile.InviterID = "U-" + strconv.FormatInt(*user.ReferredByUserID, 10)
		}
		detail.User = profile

		subscription, err := store.GetSubscription(ctx, tx, userID)
		if err != nil {
			return err
		}
		detail.Entitlement = subscription.Snapshot(now)

		devices, err := store.ListUserDevices(ctx, tx, userID)
		if err != nil {
			return err
		}
		detail.Devices = make([]AdminDeviceView, 0, len(devices))
		for _, d := range devices {
			detail.Devices = append(detail.Devices, AdminDeviceView{
				DeviceID:    d.DeviceID,
				Platform:    domain.FormatPlatform(d.Platform, d.OSVersion, d.Arch),
				AppVersion:  d.AppVersion,
				FirstSeenAt: d.FirstSeenAt,
				LastSeenAt:  d.LastSeenAt,
			})
		}

		summary, err := store.GetReferralSummary(ctx, tx, userID)
		if err != nil {
			return err
		}
		progress, err := store.ListReferralProgress(ctx, tx, userID)
		if err != nil {
			return err
		}
		items := make([]ReferralItem, 0, len(progress))
		for _, p := range progress {
			items = append(items, ReferralItem{
				EmailMasked: p.EmailMasked,
				Status:      string(p.Stage),
				BonusDays:   p.BonusDays,
				At:          p.At,
			})
		}
		detail.Referral = AdminReferralView{
			InvitedCount:   summary.ActivatedCount,
			TotalBonusDays: summary.TotalBonusDays,
			Items:          items,
		}

		orders, err := store.ListUserOrders(ctx, tx, userID, AdminUserDetailOrderLimit)
		if err != nil {
			return err
		}
		detail.Orders = viewOrders(orders)

		// 前后值留空：这是一次读取，没有状态变更，留痕的意义在于「谁在何时看了谁」。
		return store.WriteAudit(ctx, tx, store.AuditEntry{
			ActorType:  "admin",
			ActorID:    strconv.FormatInt(actor.Admin.ID, 10),
			Action:     "user.view_detail",
			TargetType: "user",
			TargetID:   strconv.FormatInt(userID, 10),
			IP:         ip,
			UserAgent:  userAgent,
		})
	})
	if err != nil {
		return AdminUserDetail{}, err
	}
	return detail, nil
}

// auditDenied 记录一次被拒的越权尝试。action 传原本要执行的动作，落库时补 `_denied` 后缀。
//
// 「谁试图越权做了什么」正是审计要回答的问题，所以每条拒绝路径都留痕，
// 而且是先写审计再返回 403。与成功路径相反，这里的审计写失败不升级为 500：
// 请求本来就没有产生任何效果、也不返回数据，把 403 变成 500 只会掩盖真正的权限判断结果。
func (s *Service) auditDenied(
	ctx context.Context, actor AdminPrincipal, action, targetType, targetID, ip, userAgent string,
) {
	deniedAction := action + "_denied"
	err := store.WriteAudit(ctx, s.Pool, store.AuditEntry{
		ActorType:  "admin",
		ActorID:    strconv.FormatInt(actor.Admin.ID, 10),
		Action:     deniedAction,
		TargetType: targetType,
		TargetID:   targetID,
		// 记下被拒时的角色，便于日后调整权限矩阵时回溯这些请求是否本该放行。
		After:     map[string]any{"actor_role": actor.Admin.Role},
		IP:        ip,
		UserAgent: userAgent,
	})
	if err != nil {
		slog.Error("写入越权访问审计失败",
			"action", deniedAction, "target_id", targetID, "error", err)
	}
}

// disableAction 返回禁用/解禁对应的审计动作名。放行与拒绝两条路径共用，避免写岔。
func disableAction(disabled bool) string {
	if disabled {
		return "user.disable"
	}
	return "user.enable"
}

// SetUserDisabled 停用或恢复用户，并留下审计记录。
// 停用会立即撤销其全部会话——「该用户将立即被登出且无法登录」。
func (s *Service) SetUserDisabled(
	ctx context.Context, actor AdminPrincipal, userID int64, disabled bool, reason, ip, userAgent string,
) error {
	action := disableAction(disabled)
	if !CanManageUsers(actor.Admin.Role) {
		s.auditDenied(ctx, actor, action, "user", strconv.FormatInt(userID, 10), ip, userAgent)
		return apperr.Forbidden()
	}

	now := s.now()

	return db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		before, err := store.GetUserByID(ctx, tx, userID)
		if err != nil {
			if errors.Is(err, store.ErrNotFound) {
				return apperr.NotFound()
			}
			return err
		}

		adminID := actor.Admin.ID
		if err := store.SetUserDisabled(ctx, tx, userID, disabled, &adminID, reason, now); err != nil {
			return err
		}
		if disabled {
			if _, err := store.RevokeUserSessions(
				ctx, tx, userID, nil, domain.RevokeAdminDisable, now,
			); err != nil {
				return err
			}
		}

		return store.WriteAudit(ctx, tx, store.AuditEntry{
			ActorType:  "admin",
			ActorID:    strconv.FormatInt(actor.Admin.ID, 10),
			Action:     action,
			TargetType: "user",
			TargetID:   strconv.FormatInt(userID, 10),
			Before:     map[string]any{"status": string(before.Status)},
			After:      map[string]any{"status": statusAfter(disabled), "reason": reason},
			IP:         ip,
			UserAgent:  userAgent,
		})
	})
}

// —— 订单与退款 ——

// AdminOrderView 是后台订单列表的一行。
type AdminOrderView struct {
	OrderNo     string     `json:"order_no"`
	EmailMasked string     `json:"email_masked"`
	AmountCents int64      `json:"amount_cents"`
	Currency    string     `json:"currency"`
	Channel     string     `json:"channel"`
	Status      string     `json:"status"`
	PaidAt      *time.Time `json:"paid_at,omitempty"`
	CreatedAt   time.Time  `json:"created_at"`
}

// ListOrders 分页查询订单。
func (s *Service) ListOrders(
	ctx context.Context, status string, page, pageSize int,
) ([]AdminOrderView, int64, error) {
	orders, total, err := store.ListOrders(ctx, s.Pool, status, pageSize, (page-1)*pageSize)
	if err != nil {
		return nil, 0, err
	}
	return viewOrders(orders), total, nil
}

// viewOrders 把订单转换为后台订单列表与用户详情共用的行表示。
func viewOrders(orders []domain.Order) []AdminOrderView {
	out := make([]AdminOrderView, 0, len(orders))
	for _, o := range orders {
		out = append(out, AdminOrderView{
			OrderNo:     o.OrderNo,
			EmailMasked: domain.MaskEmail(o.UserEmail),
			AmountCents: o.AmountCents,
			Currency:    o.Currency,
			Channel:     string(o.Channel),
			Status:      string(o.Status),
			PaidAt:      o.PaidAt,
			CreatedAt:   o.CreatedAt,
		})
	}
	return out
}

// TodayOrderTotals 返回页头常驻的当日汇总。
func (s *Service) TodayOrderTotals(ctx context.Context) (int64, int64, error) {
	today := s.now().Truncate(24 * time.Hour)
	return store.TodayOrderSummary(ctx, s.Pool, today, today.AddDate(0, 0, 1))
}

// AdminOrderExportLimit 是一次 CSV 导出最多带出的订单数。
const AdminOrderExportLimit = 5000

// ExportOrders 按状态筛选取出用于 CSV 导出的订单。
//
// 导出读的数据与列表相同，权限门槛却与写操作同级：它一次性把上千行用户邮箱
// （即便打码）落到本地文件，属于批量数据外带，不是客服日常工单需要的东西。
func (s *Service) ExportOrders(
	ctx context.Context, actor AdminPrincipal, status, ip, userAgent string,
) ([]AdminOrderView, error) {
	if !CanExportOrders(actor.Admin.Role) {
		// 目标不是某一笔订单，而是本次导出的筛选条件；status 为空串表示「全部」。
		s.auditDenied(ctx, actor, "orders.export", "orders", status, ip, userAgent)
		return nil, apperr.Forbidden()
	}

	orders, _, err := s.ListOrders(ctx, status, 1, AdminOrderExportLimit)
	if err != nil {
		return nil, err
	}

	// 成功的导出比被拒的导出更值得留痕：我们正是以「一次性把大量用户邮箱落到本地文件」
	// 为由限制这个能力的，那么真正发生了外带的那一次必须可追溯。
	// 与查看用户详情同理——记不下这次外带，就不交出数据。
	if err := store.WriteAudit(ctx, s.Pool, store.AuditEntry{
		ActorType:  "admin",
		ActorID:    strconv.FormatInt(actor.Admin.ID, 10),
		Action:     "orders.export",
		TargetType: "orders",
		TargetID:   status,
		After: map[string]any{
			"status_filter": status,
			"row_count":     len(orders),
			"truncated":     len(orders) == AdminOrderExportLimit,
		},
		IP:        ip,
		UserAgent: userAgent,
	}); err != nil {
		return nil, err
	}

	return orders, nil
}

// RefundOrder 对已支付订单发起退款，并扣回该订单对应的订阅天数。
func (s *Service) RefundOrder(
	ctx context.Context, actor AdminPrincipal, orderNo, reason, ip, userAgent string,
) (string, error) {
	if !CanRefundOrder(actor.Admin.Role) {
		s.auditDenied(ctx, actor, "order.refund", "order", orderNo, ip, userAgent)
		return "", apperr.Forbidden()
	}

	now := s.now()
	var finalStatus string
	var reject error

	err := db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		order, err := store.LockOrderByNo(ctx, tx, orderNo)
		if err != nil {
			if errors.Is(err, store.ErrNotFound) {
				return apperr.NotFound()
			}
			return err
		}
		if order.Status != domain.OrderPaid {
			return apperr.OrderNotRefundable()
		}

		if err := store.UpdateOrderStatus(
			ctx, tx, orderNo, domain.OrderRefunding, nil, nil, now,
		); err != nil {
			return err
		}
		refundID, err := store.CreateRefund(
			ctx, tx, order.ID, actor.Admin.ID, order.AmountCents, reason)
		if err != nil {
			return err
		}

		provider, err := s.Payments.Get(string(order.Channel))
		if err != nil {
			return apperr.InvalidParams().WithCause(err)
		}
		resp, err := provider.Refund(ctx, payments.RefundRequest{
			OrderNo:     orderNo,
			RefundID:    strconv.FormatInt(refundID, 10),
			AmountCents: order.AmountCents,
			Reason:      reason,
		})
		if err != nil {
			return err
		}

		finalStatus = string(domain.OrderRefunding)
		if resp.Succeeded {
			if err := store.CompleteRefund(
				ctx, tx, refundID, "succeeded", resp.ProviderRefundID, now,
			); err != nil {
				return err
			}
			if err := store.UpdateOrderStatus(
				ctx, tx, orderNo, domain.OrderRefunded, nil, nil, now,
			); err != nil {
				return err
			}
			if err := s.RevokePurchaseDays(
				ctx, tx, order.UserID, orderNo, order.PeriodMonths,
			); err != nil {
				return err
			}
			finalStatus = string(domain.OrderRefunded)
		} else {
			// 渠道明确拒绝（QA S-9）：退款单标 failed、订单恢复为已支付。
			// 绝不停在 refunding——退款回调不存在、重试会被 OrderNotRefundable
			// 拒绝，订单与退款单会永久卡死。状态变更必须提交落盘，
			// 错误改在事务外返回，否则回滚会把「恢复已支付」一并吞掉。
			if err := store.CompleteRefund(
				ctx, tx, refundID, "failed", resp.ProviderRefundID, now,
			); err != nil {
				return err
			}
			if err := store.UpdateOrderStatus(
				ctx, tx, orderNo, domain.OrderPaid, nil, nil, now,
			); err != nil {
				return err
			}
			finalStatus = string(domain.OrderPaid)
			reject = apperr.RefundDeclined()
		}

		return store.WriteAudit(ctx, tx, store.AuditEntry{
			ActorType:  "admin",
			ActorID:    strconv.FormatInt(actor.Admin.ID, 10),
			Action:     "order.refund",
			TargetType: "order",
			TargetID:   orderNo,
			Before:     map[string]any{"status": string(domain.OrderPaid)},
			After:      map[string]any{"status": finalStatus, "reason": reason},
			IP:         ip,
			UserAgent:  userAgent,
		})
	})
	if reject != nil {
		return finalStatus, reject
	}

	return finalStatus, err
}

// —— 运营配置 ——

// UpdateOpsConfig 批量写入运营配置并逐项记录前后值。
func (s *Service) UpdateOpsConfig(
	ctx context.Context, actor AdminPrincipal, values map[string]any, ip, userAgent string,
) (store.OpsConfig, error) {
	if !CanEditOpsConfig(actor.Admin.Role) {
		// 放行时每个 key 一条审计；被拒时本次提交整体没有发生，写一条即可，
		// target 用排序后的 key 列表，回溯时仍能看出他想改哪几项。
		s.auditDenied(ctx, actor, "ops_config.update", "ops_config",
			strings.Join(slices.Sorted(maps.Keys(values)), ","), ip, userAgent)
		return store.OpsConfig{}, apperr.Forbidden()
	}

	now := s.now()
	var updated store.OpsConfig

	err := db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		for key, value := range values {
			previous, err := store.SetOpsConfig(ctx, tx, key, value, actor.Admin.ID, now)
			if err != nil {
				return err
			}
			if err := store.WriteAudit(ctx, tx, store.AuditEntry{
				ActorType:  "admin",
				ActorID:    strconv.FormatInt(actor.Admin.ID, 10),
				Action:     "ops_config.update",
				TargetType: "ops_config",
				TargetID:   key,
				Before:     map[string]any{"value": rawOrNil(previous)},
				After:      map[string]any{"value": value},
				IP:         ip,
				UserAgent:  userAgent,
			}); err != nil {
				return err
			}
		}

		var err error
		updated, err = store.LoadOpsConfig(ctx, tx)
		return err
	})

	return updated, err
}

// OpsConfig 读取当前运营配置。
func (s *Service) OpsConfig(ctx context.Context) (store.OpsConfig, error) {
	return store.LoadOpsConfig(ctx, s.Pool)
}

// AuditLogs 分页读取审计日志，actor 与 action 为空串时不筛选。
func (s *Service) AuditLogs(
	ctx context.Context, actor, action string, page, pageSize int,
) ([]store.AuditRecord, int64, error) {
	return store.ListAuditLogs(ctx, s.Pool, actor, action, pageSize, (page-1)*pageSize)
}

func floatp(v float64) *float64 { return &v }

func statusAfter(disabled bool) string {
	if disabled {
		return string(domain.UserDisabled)
	}
	return string(domain.UserActive)
}

func sourceLabel(source string) string {
	switch domain.RegistrationSource(source) {
	case domain.SourceInvite:
		return "邀请"
	case domain.SourceOther:
		return "其他渠道"
	default:
		return "自然流量"
	}
}

func rawOrNil(raw []byte) any {
	if len(raw) == 0 {
		return nil
	}
	return string(raw)
}
