package service

import (
	"context"
	"errors"
	"fmt"
	"net/mail"
	"strings"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/apperr"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/db"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/domain"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/security"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/store"
)

// dummyPasswordHash 用于邮箱不存在时消耗与真实校验相当的时间，
// 避免通过响应耗时区分「邮箱不存在」与「密码错误」，配合统一文案共同防枚举。
const dummyPasswordHash = "$argon2id$v=19$m=65536,t=3,p=2$" +
	"YWFhYWFhYWFhYWFhYWFhYQ$Zm9vYmFyZm9vYmFyZm9vYmFyZm9vYmFyZm9vYmFyZm8"

// RegisterInput 是注册请求。
type RegisterInput struct {
	Email        string
	Password     string
	ReferralCode string // 来自 cch_ref cookie，用户无需手输
	VisitorID    *uuid.UUID
	UTMSource    string
	IP           string
	UserAgent    string
}

// RegisterResult 是注册结果。注册成功不发放任何会话，用户必须先验证邮箱。
type RegisterResult struct {
	Email string `json:"email"`
	Next  string `json:"next"`
	// DevCode 仅在非生产环境回传验证码，便于本地联调；生产环境恒为空。
	DevCode string `json:"dev_code,omitempty"`
}

// Register 创建账号并发送 6 位验证码。
func (s *Service) Register(ctx context.Context, in RegisterInput) (RegisterResult, error) {
	email := domain.NormalizeEmail(in.Email)
	if !validEmail(email) {
		return RegisterResult{}, apperr.EmailInvalid()
	}
	if !security.ValidatePassword(in.Password) {
		return RegisterResult{}, apperr.PasswordTooWeak()
	}
	if ok, retry := s.Limiter.Allow("register:ip:"+in.IP, RuleRegisterByIP); !ok {
		return RegisterResult{}, apperr.RateLimited(retry)
	}

	passwordHash, err := s.Hasher.Hash(in.Password)
	if err != nil {
		return RegisterResult{}, err
	}

	code, err := security.NumericCode(VerificationCodeLength)
	if err != nil {
		return RegisterResult{}, err
	}

	now := s.now()
	err = db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		if _, err := store.GetUserByEmail(ctx, tx, email); err == nil {
			return apperr.EmailTaken()
		} else if !errors.Is(err, store.ErrNotFound) {
			return err
		}

		source, inviter := s.resolveAttributionSource(ctx, tx, in)

		user, err := store.CreateUser(ctx, tx, store.CreateUserParams{
			Email:        email,
			PasswordHash: passwordHash,
			Source:       source,
			ReferredBy:   inviter,
			SignupIP:     in.IP,
			UserAgent:    in.UserAgent,
		})
		if err != nil {
			if store.IsUniqueViolation(err) {
				return apperr.EmailTaken()
			}
			return err
		}

		// 每位用户注册即拥有自己的邀请码，账户中心可直接展示。
		if err := s.assignReferralCode(ctx, tx, user.ID); err != nil {
			return err
		}

		// 归因第二步：记录「已注册」。试用与奖励要等首次登录 APP 才结算。
		if inviter != nil {
			if err := store.CreateAttribution(
				ctx, tx, in.ReferralCode, *inviter, user.ID, in.VisitorID,
			); err != nil && !store.IsUniqueViolation(err) {
				return err
			}
		}

		if err := store.UpsertVerificationCode(ctx, tx, user.ID, store.PurposeSignup, email,
			security.HashCode(code, s.Cfg.CodePepper), now.Add(VerificationCodeTTL), now); err != nil {
			return err
		}

		return store.EnqueueEmail(ctx, tx, email, store.TemplateVerifyCode, map[string]any{
			"code":        code,
			"expires_in":  int(VerificationCodeTTL.Minutes()),
			"target_name": email,
		})
	})
	if err != nil {
		return RegisterResult{}, err
	}

	return RegisterResult{Email: email, Next: "verify_email", DevCode: s.devCode(code)}, nil
}

// resolveAttributionSource 解析注册来源与邀请者。邀请码无效或指向自己时静默降级，
// 不阻断注册转化（对应 4.4「邀请码失效仍保留正常注册入口」）。
func (s *Service) resolveAttributionSource(
	ctx context.Context, q store.Querier, in RegisterInput,
) (domain.RegistrationSource, *int64) {
	if in.ReferralCode != "" {
		if rc, err := store.GetReferralCode(ctx, q, in.ReferralCode); err == nil {
			inviter := rc.UserID
			return domain.SourceInvite, &inviter
		}
	}
	if in.UTMSource != "" {
		return domain.SourceOther, nil
	}
	return domain.SourceOrganic, nil
}

// assignReferralCode 为用户生成唯一邀请码，极小概率的碰撞通过重试解决。
func (s *Service) assignReferralCode(ctx context.Context, q store.Querier, userID int64) error {
	for range 5 {
		code, err := security.ReferralCode(ReferralCodeLength)
		if err != nil {
			return err
		}
		err = store.CreateReferralCode(ctx, q, userID, code)
		if err == nil {
			return nil
		}
		if !store.IsUniqueViolation(err) {
			return err
		}
	}
	return fmt.Errorf("service: 生成邀请码连续冲突")
}

// VerifyEmailInput 是邮箱验证请求。
type VerifyEmailInput struct {
	Email      string
	Code       string
	IP         string
	UserAgent  string
	DeviceName string
}

// VerifyEmail 校验验证码、激活账号并建立官网会话。
func (s *Service) VerifyEmail(ctx context.Context, in VerifyEmailInput) (domain.User, TokenPair, error) {
	now := s.now()
	var user domain.User
	var pair TokenPair

	err := db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		u, err := store.GetUserByEmail(ctx, tx, in.Email)
		if err != nil {
			if errors.Is(err, store.ErrNotFound) {
				// 邮箱不存在与「没有有效验证码」返回一致的结果，不泄露账号是否存在。
				return apperr.CodeExpired()
			}
			return err
		}
		if u.Status == domain.UserDisabled {
			return apperr.AccountDisabled()
		}

		// 账号已激活时不存在未消费的验证码，这里自然返回「已过期」，引导用户去登录。
		record, err := store.GetActiveVerificationCode(ctx, tx, u.ID, store.PurposeSignup)
		if err != nil {
			if errors.Is(err, store.ErrNotFound) {
				return apperr.CodeExpired()
			}
			return err
		}
		if !record.ExpiresAt.After(now) || record.AttemptsRemaining() == 0 {
			return apperr.CodeExpired()
		}

		if record.CodeHash != security.HashCode(in.Code, s.Cfg.CodePepper) {
			remaining, err := store.IncrementVerificationAttempts(ctx, tx, record.ID)
			if err != nil {
				return err
			}
			if remaining == 0 {
				return apperr.CodeExpired()
			}
			return apperr.CodeInvalid(remaining)
		}

		if err := store.ConsumeVerificationCode(ctx, tx, record.ID, now); err != nil {
			return err
		}
		if err := store.ActivateUser(ctx, tx, u.ID, now); err != nil {
			return err
		}
		if err := store.RecordActivity(ctx, tx, u.ID, now); err != nil {
			return err
		}

		pair, err = s.issueSession(ctx, tx, store.CreateSessionFamilyParams{
			UserID:     u.ID,
			Client:     domain.ClientWeb,
			DeviceName: describeBrowser(in.UserAgent),
			Platform:   "browser",
			UserAgent:  in.UserAgent,
			IP:         in.IP,
		}, scopeForClient(domain.ClientWeb))
		if err != nil {
			return err
		}

		user, err = store.GetUserByID(ctx, tx, u.ID)
		return err
	})

	return user, pair, err
}

// ResendVerificationCode 重发注册验证码。
//
// 无论邮箱是否存在都返回成功（含冷却秒数），不泄露账号是否注册。
func (s *Service) ResendVerificationCode(ctx context.Context, email, ip string) (int, string, error) {
	if ok, retry := s.Limiter.Allow("resend:ip:"+ip, RuleResendByIP); !ok {
		return 0, "", apperr.RateLimited(retry)
	}

	now := s.now()
	cooldown := int(VerificationResendCooldown.Seconds())
	var issued string

	err := db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		u, err := store.GetUserByEmail(ctx, tx, email)
		if err != nil {
			if errors.Is(err, store.ErrNotFound) {
				return nil // 静默成功
			}
			return err
		}
		if u.Status != domain.UserPendingEmail {
			return nil // 已验证或已停用，同样静默成功
		}

		// 60 秒重发冷却：未到期时直接返回剩余秒数，且不生成新验证码。
		if existing, err := store.GetActiveVerificationCode(
			ctx, tx, u.ID, store.PurposeSignup,
		); err == nil {
			if elapsed := now.Sub(existing.LastSentAt); elapsed < VerificationResendCooldown {
				cooldown = int((VerificationResendCooldown - elapsed).Seconds())
				return nil
			}
		} else if !errors.Is(err, store.ErrNotFound) {
			return err
		}

		code, err := security.NumericCode(VerificationCodeLength)
		if err != nil {
			return err
		}
		if err := store.UpsertVerificationCode(ctx, tx, u.ID, store.PurposeSignup, u.Email,
			security.HashCode(code, s.Cfg.CodePepper), now.Add(VerificationCodeTTL), now); err != nil {
			return err
		}
		issued = code

		return store.EnqueueEmail(ctx, tx, u.Email, store.TemplateVerifyCode, map[string]any{
			"code":       code,
			"expires_in": int(VerificationCodeTTL.Minutes()),
		})
	})

	return cooldown, s.devCode(issued), err
}

// LoginInput 是登录请求。
type LoginInput struct {
	Email     string
	Password  string
	IP        string
	UserAgent string
}

// Login 校验凭据并建立官网会话。
//
// 失败一律返回同一个错误与同一句文案「邮箱或密码不正确。」，
// 邮箱不存在时也会执行一次等价的哈希校验以抹平耗时差异。
func (s *Service) Login(ctx context.Context, in LoginInput) (domain.User, TokenPair, error) {
	email := domain.NormalizeEmail(in.Email)

	if ok, retry := s.Limiter.Allow("login:ip:"+in.IP, RuleLoginByIP); !ok {
		return domain.User{}, TokenPair{}, apperr.RateLimited(retry)
	}
	if ok, retry := s.Limiter.Allow("login:email:"+email, RuleLoginByEmail); !ok {
		return domain.User{}, TokenPair{}, apperr.RateLimited(retry)
	}

	now := s.now()
	var user domain.User
	var pair TokenPair

	err := db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		u, err := store.GetUserByEmail(ctx, tx, email)
		if err != nil {
			if errors.Is(err, store.ErrNotFound) {
				_ = s.Hasher.Verify(in.Password, dummyPasswordHash)
				return apperr.InvalidCredentials()
			}
			return err
		}

		if u.IsLocked(now) {
			return apperr.AccountLocked(u.LockRemaining(now))
		}
		if u.Status == domain.UserDisabled {
			return apperr.AccountDisabled()
		}

		if err := s.Hasher.Verify(in.Password, u.PasswordHash); err != nil {
			if !errors.Is(err, security.ErrPasswordMismatch) {
				return err
			}
			updated, err := store.RecordLoginFailure(
				ctx, tx, u.ID, LoginFailureThreshold, LoginLockDuration, now)
			if err != nil {
				return err
			}
			if updated.IsLocked(now) {
				return apperr.AccountLocked(updated.LockRemaining(now))
			}
			return apperr.InvalidCredentials()
		}

		// 口令正确后才暴露「邮箱未验证」，避免把它变成账号存在性探针。
		if u.Status == domain.UserPendingEmail {
			return apperr.EmailUnverified()
		}

		if err := store.ClearLoginFailures(ctx, tx, u.ID, now); err != nil {
			return err
		}
		if err := store.RecordActivity(ctx, tx, u.ID, now); err != nil {
			return err
		}

		pair, err = s.issueSession(ctx, tx, store.CreateSessionFamilyParams{
			UserID:     u.ID,
			Client:     domain.ClientWeb,
			DeviceName: describeBrowser(in.UserAgent),
			Platform:   "browser",
			UserAgent:  in.UserAgent,
			IP:         in.IP,
		}, scopeForClient(domain.ClientWeb))
		if err != nil {
			return err
		}

		user = u
		return nil
	})
	if err != nil {
		return domain.User{}, TokenPair{}, err
	}

	s.Limiter.Reset("login:email:" + email)
	return user, pair, nil
}

// RequestPasswordReset 发送重设链接。
//
// 无论邮箱是否注册都返回成功，由调用方渲染 6.2 节的固定回执文案。
func (s *Service) RequestPasswordReset(ctx context.Context, email, ip string) (string, error) {
	if ok, retry := s.Limiter.Allow("forgot:ip:"+ip, RuleForgotByIP); !ok {
		return "", apperr.RateLimited(retry)
	}

	now := s.now()
	var issued string

	err := db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		u, err := store.GetUserByEmail(ctx, tx, email)
		if err != nil {
			if errors.Is(err, store.ErrNotFound) {
				return nil
			}
			return err
		}
		if u.Status == domain.UserDisabled {
			return nil
		}

		// 60 秒内重复申请不再发信，但对外仍表现为成功。
		if last, err := store.LatestPasswordResetRequest(ctx, tx, u.ID); err == nil {
			if now.Sub(last) < VerificationResendCooldown {
				return nil
			}
		} else if !errors.Is(err, store.ErrNotFound) {
			return err
		}

		token, err := security.RandomToken(PasswordResetTokenBytes)
		if err != nil {
			return err
		}
		if err := store.CreatePasswordResetToken(ctx, tx, u.ID,
			security.HashToken(token), ip, now.Add(PasswordResetTTL)); err != nil {
			return err
		}
		issued = token

		return store.EnqueueEmail(ctx, tx, u.Email, store.TemplatePasswordReset, map[string]any{
			"reset_url":  fmt.Sprintf("%s/reset-password?token=%s", s.Cfg.PublicURL, token),
			"expires_in": int(PasswordResetTTL.Minutes()),
		})
	})

	return s.devCode(issued), err
}

// InspectPasswordResetToken 校验重设链接是否仍然有效，供落地页渲染骨架/失效态。
func (s *Service) InspectPasswordResetToken(ctx context.Context, token string) (string, error) {
	record, err := store.GetValidPasswordResetToken(ctx, s.Pool, security.HashToken(token), s.now())
	if err != nil {
		if errors.Is(err, store.ErrNotFound) {
			return "", apperr.ResetLinkInvalid()
		}
		return "", err
	}

	user, err := store.GetUserByID(ctx, s.Pool, record.UserID)
	if err != nil {
		return "", err
	}
	return domain.MaskEmail(user.Email), nil
}

// ResetPassword 使用一次性令牌重设密码，并撤销该账号的全部会话。
func (s *Service) ResetPassword(ctx context.Context, token, newPassword string) error {
	if !security.ValidatePassword(newPassword) {
		return apperr.PasswordTooWeak()
	}

	hash, err := s.Hasher.Hash(newPassword)
	if err != nil {
		return err
	}

	now := s.now()
	return db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		record, err := store.GetValidPasswordResetToken(ctx, tx, security.HashToken(token), now)
		if err != nil {
			if errors.Is(err, store.ErrNotFound) {
				return apperr.ResetLinkInvalid()
			}
			return err
		}
		// 一次性：并发使用同一链接时只有一方成功。
		if err := store.ConsumePasswordResetToken(ctx, tx, record.ID, now); err != nil {
			if errors.Is(err, store.ErrNotFound) {
				return apperr.ResetLinkInvalid()
			}
			return err
		}
		if err := store.UpdatePasswordHash(ctx, tx, record.UserID, hash, now); err != nil {
			return err
		}

		_, err = store.RevokeUserSessions(ctx, tx, record.UserID, nil, domain.RevokePasswordReset, now)
		return err
	})
}

// devCode 只在非生产环境回传验证码/令牌，方便本地与自动化联调。
func (s *Service) devCode(code string) string {
	if s.Cfg.Env == "prod" {
		return ""
	}
	return code
}

func validEmail(email string) bool {
	if email == "" || len(email) > 254 || strings.ContainsAny(email, " \t\r\n") {
		return false
	}
	addr, err := mail.ParseAddress(email)
	return err == nil && addr.Address == email
}
