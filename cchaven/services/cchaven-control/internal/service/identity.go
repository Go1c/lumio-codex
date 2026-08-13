package service

import (
	"context"
	"errors"
	"log/slog"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/apperr"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/db"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/domain"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/store"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/sub2api"
)

// FederatedPasswordPlaceholder 是影子账号的 password_hash 占位值。
//
// users.password_hash 是 NOT NULL，而 Sub2API 托管的账号在本库没有口令。
// 这个串不是合法的 argon2 编码，任何校验都会失败——即便将来有人误开本地登录，
// 也不可能凭它登进来。
const FederatedPasswordPlaceholder = "!sub2api-managed"

// IdentityInput 是一次 Sub2API 令牌鉴权的输入。
//
// 邀请相关字段只在「这个 Sub2API 用户第一次出现在 CC」时才有意义：
// 开户是本服务唯一还能做归因的时刻，注册已经不在这里发生了。
type IdentityInput struct {
	Token        string
	ReferralCode string
	VisitorID    *uuid.UUID
	IP           string
	UserAgent    string
}

// AuthenticateSub2API 用 Sub2API 令牌解析调用者身份。
//
// 身份真源在 Sub2API：先拿令牌换回 {id, email, status}，再把它映射到本地的
// CC 影子账号（首次出现即开户）。本地的 disabled 状态仍然有效——运营在后台
// 封禁某个用户时，不需要也不应该去动 Sub2API。
func (s *Service) AuthenticateSub2API(ctx context.Context, in IdentityInput) (Principal, error) {
	if s.Sub2API == nil {
		return Principal{}, apperr.IdentityUnavailable()
	}

	identity, err := s.Sub2API.Verify(ctx, in.Token)
	switch {
	case errors.Is(err, sub2api.ErrInvalidToken):
		return Principal{}, apperr.Unauthorized()
	case errors.Is(err, sub2api.ErrUnavailable):
		// 记一条日志：这条路径一旦持续出现，就是账号中心出了问题。
		slog.Error("无法向 Sub2API 校验令牌", "error", err)
		return Principal{}, apperr.IdentityUnavailable().WithCause(err)
	case err != nil:
		return Principal{}, err
	}
	if !identity.Active() {
		return Principal{}, apperr.AccountDisabled()
	}

	user, err := s.LinkSub2APIIdentity(ctx, identity, in)
	if err != nil {
		return Principal{}, err
	}
	if user.Status == domain.UserDisabled {
		return Principal{}, apperr.AccountDisabled()
	}

	// Sub2API 令牌不对应本地会话族，SessionID 留空：
	// 「当前设备」只对 APP 自己签发的会话有意义。
	return Principal{User: user, Client: domain.ClientWeb, Scope: "profile"}, nil
}

// LinkSub2APIIdentity 把 Sub2API 身份映射到本地账号，必要时开户。
//
// 同一个用户的多个请求可能同时首次到达（桌面端启动会并发打好几个接口），
// 此时只有一方能插入成功，另一方撞唯一索引。这属于预期竞争，重试一次即可
// 走到「映射已存在」的分支，不该让用户看见 500。
func (s *Service) LinkSub2APIIdentity(
	ctx context.Context, identity sub2api.Identity, in IdentityInput,
) (domain.User, error) {
	user, err := s.linkSub2APIIdentityOnce(ctx, identity, in)
	if err != nil && store.IsUniqueViolation(err) {
		return s.linkSub2APIIdentityOnce(ctx, identity, in)
	}
	return user, err
}

func (s *Service) linkSub2APIIdentityOnce(
	ctx context.Context, identity sub2api.Identity, in IdentityInput,
) (domain.User, error) {
	now := s.now()
	var user domain.User

	err := db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		mapping, err := store.GetIdentityBySub2APIUserID(ctx, tx, identity.ID)
		if err == nil {
			user, err = store.GetUserByID(ctx, tx, mapping.UserID)
			if err != nil {
				return err
			}
			if err := store.TouchIdentity(ctx, tx, identity.ID, now); err != nil {
				return err
			}
			user, err = s.syncFederatedEmail(ctx, tx, user, identity.Email, now)
			return err
		}
		if !errors.Is(err, store.ErrNotFound) {
			return err
		}

		// 老用户：本库已有同邮箱账号，直接认领，不新建、不动历史数据。
		if identity.Email != "" {
			existing, err := store.GetUserByEmail(ctx, tx, identity.Email)
			switch {
			case err == nil:
				user = existing
				return store.LinkIdentity(ctx, tx, identity.ID, existing.ID, identity.Email, now)
			case !errors.Is(err, store.ErrNotFound):
				return err
			}
		}

		user, err = s.provisionFederatedUser(ctx, tx, identity, in, now)
		return err
	})

	return user, err
}

// provisionFederatedUser 为首次出现的 Sub2API 用户建立 CC 影子账号。
//
// 直接置为 active：邮箱已由 Sub2API 验证过，本地再走一遍验证码没有意义。
func (s *Service) provisionFederatedUser(
	ctx context.Context, tx pgx.Tx, identity sub2api.Identity, in IdentityInput, now time.Time,
) (domain.User, error) {
	source, inviter := s.resolveAttributionSource(ctx, tx, RegisterInput{
		ReferralCode: in.ReferralCode,
		VisitorID:    in.VisitorID,
	})

	user, err := store.CreateUser(ctx, tx, store.CreateUserParams{
		Email:        identity.Email,
		PasswordHash: FederatedPasswordPlaceholder,
		Source:       source,
		ReferredBy:   inviter,
		SignupIP:     in.IP,
		UserAgent:    in.UserAgent,
	})
	if err != nil {
		return domain.User{}, err
	}
	if err := store.ActivateUser(ctx, tx, user.ID, now); err != nil {
		return domain.User{}, err
	}
	if err := s.assignReferralCode(ctx, tx, user.ID); err != nil {
		return domain.User{}, err
	}
	if inviter != nil {
		if err := store.CreateAttribution(
			ctx, tx, in.ReferralCode, *inviter, user.ID, in.VisitorID,
		); err != nil && !store.IsUniqueViolation(err) {
			return domain.User{}, err
		}
	}
	if err := store.LinkIdentity(ctx, tx, identity.ID, user.ID, identity.Email, now); err != nil {
		return domain.User{}, err
	}

	return store.GetUserByID(ctx, tx, user.ID)
}

// syncFederatedEmail 让本地邮箱跟随 Sub2API。
//
// 邮箱在账号中心改过之后，本地的展示与后台检索都得跟上。撞上本地唯一索引时
// 保留旧值并告警：那说明存在两条本该合并的历史账号，需要人工介入，
// 但不该因此让用户登不进来。
func (s *Service) syncFederatedEmail(
	ctx context.Context, tx pgx.Tx, user domain.User, upstreamEmail string, now time.Time,
) (domain.User, error) {
	normalized := domain.NormalizeEmail(upstreamEmail)
	if normalized == "" || normalized == domain.NormalizeEmail(user.Email) {
		return user, nil
	}

	if err := store.UpdateEmail(ctx, tx, user.ID, normalized, now); err != nil {
		if store.IsUniqueViolation(err) {
			slog.Warn("Sub2API 邮箱与本地另一账号冲突，保留本地邮箱",
				"user_id", user.ID, "local_email", domain.MaskEmail(user.Email))
			return user, nil
		}
		return domain.User{}, err
	}
	user.Email = normalized
	return user, nil
}
