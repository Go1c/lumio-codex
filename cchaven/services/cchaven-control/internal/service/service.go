// Package service 承载业务逻辑：编排仓储、实施安全策略、维护跨表不变量。
package service

import (
	"time"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/config"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/db"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/payments"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/ratelimit"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/security"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/sub2api"
)

// 安全策略常量，全部来自交互设计 6.2 节与 3.1 / 3.3 节。
const (
	// VerificationCodeLength 验证码位数。
	VerificationCodeLength = 6
	// VerificationCodeTTL 验证码有效期。
	VerificationCodeTTL = 10 * time.Minute
	// VerificationMaxAttempts 验证码最大尝试次数。
	VerificationMaxAttempts = 5
	// VerificationResendCooldown 验证码重发冷却。
	//
	// 规范 3.1 与 6.2 均为 60 秒；原型把它压缩到 10 秒仅为演示方便，
	// 并在页面上自注「规格为 60 秒」，故后端以 60 秒为准。
	VerificationResendCooldown = 60 * time.Second

	// PasswordResetTokenBytes 重设密码令牌的随机字节数。
	PasswordResetTokenBytes = 32
	// PasswordResetTTL 重设链接有效期，一次性。
	PasswordResetTTL = 20 * time.Minute

	// LoginFailureThreshold 触发锁定的连续登录失败次数。
	LoginFailureThreshold = 5
	// LoginLockDuration 登录锁定时长。
	LoginLockDuration = 15 * time.Minute

	// AuthorizationCodeTTL OAuth 授权码有效期。
	AuthorizationCodeTTL = 5 * time.Minute
	// AuthorizationCodeBytes OAuth 授权码的随机字节数。
	AuthorizationCodeBytes = 32
	// RefreshTokenBytes refresh token 的随机字节数。
	RefreshTokenBytes = 32

	// ReferralCodeLength 邀请码长度。
	ReferralCodeLength = 8
	// AccountDeletionGracePeriod 注销冷静期。
	AccountDeletionGracePeriod = 7 * 24 * time.Hour

	// AdminLoginFailureThreshold 管理员登录失败锁定阈值。
	AdminLoginFailureThreshold = 5
	// AdminLoginLockDuration 管理员登录锁定时长。
	AdminLoginLockDuration = 15 * time.Minute
)

// 限频规则。IP 维度防刷，邮箱维度防定向骚扰。
var (
	// RuleRegisterByIP 同一 IP 的注册频率。
	RuleRegisterByIP = ratelimit.Rule{Limit: 5, Window: time.Minute}
	// RuleLoginByIP 同一 IP 的登录尝试频率。
	RuleLoginByIP = ratelimit.Rule{Limit: 10, Window: time.Minute}
	// RuleLoginByEmail 同一邮箱的登录尝试频率。
	RuleLoginByEmail = ratelimit.Rule{Limit: 10, Window: 5 * time.Minute}
	// RuleForgotByIP 同一 IP 申请重设密码的频率。
	RuleForgotByIP = ratelimit.Rule{Limit: 5, Window: 10 * time.Minute}
	// RuleResendByIP 同一 IP 重发验证码的频率。
	RuleResendByIP = ratelimit.Rule{Limit: 10, Window: 10 * time.Minute}
	// RuleAdminTOTPByIP 管理端 TOTP 验证码的尝试频率。
	//
	// 口令锁定管「按账号」的穷举，这里管「按来源」的穷举（QA S-1）：两个维度
	// 合起来，拿到口令的攻击者也无法对 6 位验证码做小时级在线暴破。
	RuleAdminTOTPByIP = ratelimit.Rule{Limit: 10, Window: time.Minute}
	// RulePublicReadByIP 公开只读接口（价格配置、邀请落地、归因回执）的频率。
	//
	// 配额给得宽松：这些接口支撑的是官网首屏，正常访客一次会话只打几次，
	// 而办公网/校园网常常整栋楼共用一个出口 IP。误伤真实访客的代价，
	// 远大于这几条廉价查询被刷的代价。
	RulePublicReadByIP = ratelimit.Rule{Limit: 300, Window: time.Minute}
)

// Service 是应用服务容器。
type Service struct {
	Pool     *db.Pool
	Cfg      config.Config
	Hasher   *security.Hasher
	Tokens   *security.TokenIssuer
	Cipher   *security.Cipher
	Limiter  *ratelimit.Limiter
	Payments *payments.Registry
	// Sub2API 是终端用户身份的真源客户端；测试可替换为指向假上游的实例。
	Sub2API *sub2api.Client

	// Now 允许测试注入可控时钟；生产环境为 time.Now。
	Now func() time.Time
}

// New 构造服务容器。
func New(
	pool *db.Pool, cfg config.Config,
	hasher *security.Hasher, cipher *security.Cipher, registry *payments.Registry,
) *Service {
	return &Service{
		Pool:     pool,
		Cfg:      cfg,
		Hasher:   hasher,
		Tokens:   security.NewTokenIssuer(cfg.JWTSecret, cfg.AccessTokenTTL),
		Cipher:   cipher,
		Limiter:  ratelimit.New(),
		Payments: registry,
		Sub2API: sub2api.New(sub2api.Options{
			BaseURL:  cfg.Sub2APIBase,
			CacheTTL: cfg.Sub2APICacheTTL,
		}),
		Now: time.Now,
	}
}

// now 返回当前时间（UTC），统一入口便于测试替换时钟。
func (s *Service) now() time.Time { return s.Now().UTC() }
