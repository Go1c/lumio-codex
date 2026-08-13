package service

import (
	"context"
	"errors"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/apperr"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/db"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/domain"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/security"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/store"
)

// TokenPair 是一次会话签发的结果。
type TokenPair struct {
	AccessToken  string
	RefreshToken string
	ExpiresIn    int
	SessionID    uuid.UUID
}

// Principal 是通过鉴权的调用者身份。
type Principal struct {
	User      domain.User
	SessionID uuid.UUID
	Client    domain.SessionClient
	Scope     string
}

// issueSession 建立会话族并签发首对令牌。调用方通常已在事务中。
func (s *Service) issueSession(
	ctx context.Context, q store.Querier, p store.CreateSessionFamilyParams, scope string,
) (TokenPair, error) {
	now := s.now()

	familyID, err := store.CreateSessionFamily(ctx, q, p)
	if err != nil {
		return TokenPair{}, err
	}

	refreshToken, err := security.RandomToken(RefreshTokenBytes)
	if err != nil {
		return TokenPair{}, err
	}
	if _, err := store.CreateRefreshToken(ctx, q, familyID,
		security.HashToken(refreshToken), now.Add(s.Cfg.RefreshTokenTTL)); err != nil {
		return TokenPair{}, err
	}

	accessToken, err := s.Tokens.Issue(p.UserID, familyID, scope, now)
	if err != nil {
		return TokenPair{}, err
	}

	return TokenPair{
		AccessToken:  accessToken,
		RefreshToken: refreshToken,
		ExpiresIn:    int(s.Tokens.TTL().Seconds()),
		SessionID:    familyID,
	}, nil
}

// RefreshSession 轮换 refresh token 并签发新的 access token。
//
// 若出示的令牌此前已被轮换过，说明令牌被复制外泄，立即撤销整个会话族——
// 攻击者与真实用户都会被登出，由用户重新登录，这是 refresh rotation 的标准处置。
func (s *Service) RefreshSession(ctx context.Context, refreshToken string) (TokenPair, error) {
	if refreshToken == "" {
		return TokenPair{}, apperr.SessionExpired()
	}

	now := s.now()
	var pair TokenPair

	err := db.InTx(ctx, s.Pool, func(tx pgx.Tx) error {
		record, err := store.GetRefreshTokenByHash(ctx, tx, security.HashToken(refreshToken))
		if err != nil {
			if errors.Is(err, store.ErrNotFound) {
				return apperr.SessionExpired()
			}
			return err
		}

		if record.UsedAt != nil {
			if err := store.RevokeSessionFamily(
				ctx, tx, record.FamilyID, domain.RevokeReuseDetected, now,
			); err != nil && !errors.Is(err, store.ErrNotFound) {
				return err
			}
			return apperr.SessionExpired()
		}
		if record.RevokedAt != nil || !record.ExpiresAt.After(now) {
			return apperr.SessionExpired()
		}

		family, err := store.GetActiveSessionFamily(ctx, tx, record.FamilyID)
		if err != nil {
			if errors.Is(err, store.ErrNotFound) {
				return apperr.SessionExpired()
			}
			return err
		}

		user, err := store.GetUserByID(ctx, tx, family.UserID)
		if err != nil {
			return err
		}
		if user.Status != domain.UserActive {
			return apperr.AccountDisabled()
		}

		nextToken, err := security.RandomToken(RefreshTokenBytes)
		if err != nil {
			return err
		}
		nextID, err := store.CreateRefreshToken(ctx, tx, family.ID,
			security.HashToken(nextToken), now.Add(s.Cfg.RefreshTokenTTL))
		if err != nil {
			return err
		}
		// 并发轮换时只有一方能把旧令牌标记为已用，另一方拿到 ErrNotFound 并被拒绝。
		if err := store.MarkRefreshTokenUsed(ctx, tx, record.ID, nextID, now); err != nil {
			if errors.Is(err, store.ErrNotFound) {
				return apperr.SessionExpired()
			}
			return err
		}
		if err := store.TouchSessionFamily(ctx, tx, family.ID, now); err != nil {
			return err
		}

		accessToken, err := s.Tokens.Issue(user.ID, family.ID, scopeForClient(family.Client), now)
		if err != nil {
			return err
		}

		pair = TokenPair{
			AccessToken:  accessToken,
			RefreshToken: nextToken,
			ExpiresIn:    int(s.Tokens.TTL().Seconds()),
			SessionID:    family.ID,
		}
		return nil
	})

	return pair, err
}

// AuthenticateAccess 校验 access token 并解析调用者身份。
//
// 除了验签，每次都回查会话族与账号状态：这样管理员禁用用户、用户撤销设备、
// 修改密码撤销其他会话，都能立即生效，而不必等 access token 自然过期。
func (s *Service) AuthenticateAccess(ctx context.Context, token string) (Principal, error) {
	claims, err := s.Tokens.Parse(token)
	if err != nil {
		return Principal{}, apperr.SessionExpired()
	}

	family, err := store.GetActiveSessionFamily(ctx, s.Pool, claims.SessionID)
	if err != nil {
		if errors.Is(err, store.ErrNotFound) {
			return Principal{}, apperr.SessionExpired()
		}
		return Principal{}, err
	}

	user, err := store.GetUserByID(ctx, s.Pool, family.UserID)
	if err != nil {
		if errors.Is(err, store.ErrNotFound) {
			return Principal{}, apperr.SessionExpired()
		}
		return Principal{}, err
	}
	if user.Status == domain.UserDisabled {
		return Principal{}, apperr.AccountDisabled()
	}
	if user.Status != domain.UserActive {
		return Principal{}, apperr.SessionExpired()
	}

	return Principal{
		User:      user,
		SessionID: family.ID,
		Client:    family.Client,
		Scope:     claims.Scope,
	}, nil
}

// Logout 撤销当前会话族。
func (s *Service) Logout(ctx context.Context, sessionID uuid.UUID) error {
	err := store.RevokeSessionFamily(ctx, s.Pool, sessionID, domain.RevokeUserLogout, s.now())
	if errors.Is(err, store.ErrNotFound) {
		return nil // 已经登出，视为成功
	}
	return err
}

// SessionView 是「登录设备与授权」列表中的一项。
//
// 时间一律以 RFC3339 下发，由前端按 6.5 节格式（YYYY年M月D日 / 相对时间）渲染。
type SessionView struct {
	ID         string    `json:"id"`
	DeviceName string    `json:"device_name"`
	Kind       string    `json:"kind"`
	Platform   string    `json:"platform_detail"`
	AppVersion string    `json:"app_version,omitempty"`
	LastSeenAt time.Time `json:"last_seen_at"`
	IPRegion   string    `json:"ip_region"`
	Current    bool      `json:"current"`
}

// ListSessions 列出用户的活跃会话。
func (s *Service) ListSessions(ctx context.Context, userID int64, current uuid.UUID) ([]SessionView, error) {
	families, err := store.ListSessionFamilies(ctx, s.Pool, userID)
	if err != nil {
		return nil, err
	}

	out := make([]SessionView, 0, len(families))
	for _, f := range families {
		out = append(out, SessionView{
			ID:         f.ID.String(),
			DeviceName: f.DeviceName,
			Kind:       string(f.Client),
			Platform:   f.PlatformDetail(),
			AppVersion: f.AppVersion,
			LastSeenAt: f.LastSeenAt,
			IPRegion:   f.IPRegion,
			Current:    f.ID == current,
		})
	}
	return out, nil
}

// RevokeSession 退出指定设备。
func (s *Service) RevokeSession(ctx context.Context, userID int64, sessionID uuid.UUID) error {
	// 先确认该会话属于调用者，避免越权撤销他人设备。
	family, err := store.GetActiveSessionFamily(ctx, s.Pool, sessionID)
	if err != nil {
		if errors.Is(err, store.ErrNotFound) {
			return apperr.NotFound()
		}
		return err
	}
	if family.UserID != userID {
		return apperr.NotFound()
	}

	return store.RevokeSessionFamily(ctx, s.Pool, sessionID, domain.RevokeUserRevoke, s.now())
}

// RevokeOtherSessions 退出除当前设备外的全部会话。
func (s *Service) RevokeOtherSessions(ctx context.Context, userID int64, current uuid.UUID) (int64, error) {
	return store.RevokeUserSessions(ctx, s.Pool, userID, &current, domain.RevokeOthers, s.now())
}

func scopeForClient(client domain.SessionClient) string {
	if client == domain.ClientApp {
		return "profile workspace offline_access"
	}
	return "profile"
}

// describeBrowser 从 User-Agent 粗略推断「浏览器 · 系统」，用于设备列表展示。
// 只做展示用途，不参与任何安全判断，故不必精确。
func describeBrowser(userAgent string) string {
	ua := strings.ToLower(userAgent)

	browser := "浏览器"
	switch {
	case strings.Contains(ua, "edg/"):
		browser = "Edge"
	case strings.Contains(ua, "chrome/") && !strings.Contains(ua, "chromium"):
		browser = "Chrome"
	case strings.Contains(ua, "firefox/"):
		browser = "Firefox"
	case strings.Contains(ua, "safari/"):
		browser = "Safari"
	}

	system := "未知系统"
	switch {
	case strings.Contains(ua, "mac os x"), strings.Contains(ua, "macintosh"):
		system = "macOS"
	case strings.Contains(ua, "windows"):
		system = "Windows"
	case strings.Contains(ua, "iphone"), strings.Contains(ua, "ipad"):
		system = "iOS"
	case strings.Contains(ua, "android"):
		system = "Android"
	case strings.Contains(ua, "linux"):
		system = "Linux"
	}
	return browser + " · " + system
}
