package security

import (
	"errors"
	"fmt"
	"strconv"
	"time"

	"github.com/golang-jwt/jwt/v5"
	"github.com/google/uuid"
)

// ErrInvalidToken 表示 access token 签名非法或已过期。
var ErrInvalidToken = errors.New("security: access token 无效")

const jwtIssuer = "cchaven-control"

// AccessClaims 是 access token 的载荷。
//
// sid 指向会话族：每次受保护请求都会用它回查 session_families，
// 因此管理员禁用用户或用户撤销设备后能立即生效，而不必等 token 自然过期。
type AccessClaims struct {
	UserID    int64
	SessionID uuid.UUID
	Scope     string
	TokenID   uuid.UUID
	ExpiresAt time.Time
}

// TokenIssuer 使用 HMAC-SHA256 签发与校验 access token。
type TokenIssuer struct {
	secret []byte
	ttl    time.Duration
}

// NewTokenIssuer 构造签发器。
func NewTokenIssuer(secret []byte, ttl time.Duration) *TokenIssuer {
	return &TokenIssuer{secret: secret, ttl: ttl}
}

// TTL 返回 access token 有效期。
func (t *TokenIssuer) TTL() time.Duration { return t.ttl }

// Issue 签发 access token。
func (t *TokenIssuer) Issue(userID int64, sessionID uuid.UUID, scope string, now time.Time) (string, error) {
	claims := jwt.MapClaims{
		"iss":   jwtIssuer,
		"sub":   strconv.FormatInt(userID, 10),
		"sid":   sessionID.String(),
		"jti":   uuid.NewString(),
		"scope": scope,
		"iat":   now.Unix(),
		"exp":   now.Add(t.ttl).Unix(),
	}

	signed, err := jwt.NewWithClaims(jwt.SigningMethodHS256, claims).SignedString(t.secret)
	if err != nil {
		return "", fmt.Errorf("security: 签发 access token 失败: %w", err)
	}
	return signed, nil
}

// Parse 校验签名与有效期并解出载荷。
func (t *TokenIssuer) Parse(token string) (AccessClaims, error) {
	parsed, err := jwt.Parse(token,
		func(*jwt.Token) (any, error) { return t.secret, nil },
		jwt.WithValidMethods([]string{jwt.SigningMethodHS256.Alg()}),
		jwt.WithIssuer(jwtIssuer),
		jwt.WithExpirationRequired(),
	)
	if err != nil || !parsed.Valid {
		return AccessClaims{}, ErrInvalidToken
	}

	claims, ok := parsed.Claims.(jwt.MapClaims)
	if !ok {
		return AccessClaims{}, ErrInvalidToken
	}

	sub, _ := claims["sub"].(string)
	userID, err := strconv.ParseInt(sub, 10, 64)
	if err != nil {
		return AccessClaims{}, ErrInvalidToken
	}

	sid, _ := claims["sid"].(string)
	sessionID, err := uuid.Parse(sid)
	if err != nil {
		return AccessClaims{}, ErrInvalidToken
	}

	jti, _ := claims["jti"].(string)
	tokenID, err := uuid.Parse(jti)
	if err != nil {
		return AccessClaims{}, ErrInvalidToken
	}

	exp, err := claims.GetExpirationTime()
	if err != nil || exp == nil {
		return AccessClaims{}, ErrInvalidToken
	}

	scope, _ := claims["scope"].(string)
	return AccessClaims{
		UserID:    userID,
		SessionID: sessionID,
		Scope:     scope,
		TokenID:   tokenID,
		ExpiresAt: exp.Time,
	}, nil
}
