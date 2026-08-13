// Package security 汇集口令哈希、随机令牌、JWT 与对称加密等安全原语。
package security

import (
	"crypto/rand"
	"crypto/subtle"
	"encoding/base64"
	"errors"
	"fmt"
	"strings"
	"unicode"

	"golang.org/x/crypto/argon2"
)

// ErrPasswordMismatch 表示口令与哈希不匹配。
var ErrPasswordMismatch = errors.New("security: 口令不匹配")

// Argon2Params 是 Argon2id 的代价参数。
type Argon2Params struct {
	Memory      uint32 // KiB
	Iterations  uint32
	Parallelism uint8
	SaltLength  uint32
	KeyLength   uint32
}

// DefaultArgon2Params 是生产环境参数：64 MiB / 3 轮 / 2 并行，约 50–80ms。
func DefaultArgon2Params() Argon2Params {
	return Argon2Params{Memory: 64 * 1024, Iterations: 3, Parallelism: 2, SaltLength: 16, KeyLength: 32}
}

// TestArgon2Params 是测试专用的低代价参数，避免大量用例把测试拖慢到分钟级。
// 仅供 _test 使用，切勿在生产路径引用。
func TestArgon2Params() Argon2Params {
	return Argon2Params{Memory: 8 * 1024, Iterations: 1, Parallelism: 1, SaltLength: 16, KeyLength: 32}
}

// Hasher 按给定参数生成与校验 Argon2id 口令哈希。
type Hasher struct{ params Argon2Params }

// NewHasher 构造口令哈希器。
func NewHasher(p Argon2Params) *Hasher { return &Hasher{params: p} }

// Hash 生成 PHC 格式的 Argon2id 哈希串：$argon2id$v=19$m=...,t=...,p=...$salt$hash
func (h *Hasher) Hash(password string) (string, error) {
	salt := make([]byte, h.params.SaltLength)
	if _, err := rand.Read(salt); err != nil {
		return "", fmt.Errorf("security: 生成盐失败: %w", err)
	}

	key := argon2.IDKey([]byte(password), salt,
		h.params.Iterations, h.params.Memory, h.params.Parallelism, h.params.KeyLength)

	return fmt.Sprintf("$argon2id$v=%d$m=%d,t=%d,p=%d$%s$%s",
		argon2.Version, h.params.Memory, h.params.Iterations, h.params.Parallelism,
		base64.RawStdEncoding.EncodeToString(salt),
		base64.RawStdEncoding.EncodeToString(key),
	), nil
}

// Verify 校验口令。哈希串自带参数，故旧参数生成的哈希仍可校验（便于日后提高代价）。
func (h *Hasher) Verify(password, encoded string) error {
	params, salt, want, err := decodeHash(encoded)
	if err != nil {
		return err
	}

	got := argon2.IDKey([]byte(password), salt,
		params.Iterations, params.Memory, params.Parallelism, uint32(len(want)))

	if subtle.ConstantTimeCompare(got, want) != 1 {
		return ErrPasswordMismatch
	}
	return nil
}

func decodeHash(encoded string) (Argon2Params, []byte, []byte, error) {
	parts := strings.Split(encoded, "$")
	if len(parts) != 6 || parts[1] != "argon2id" {
		return Argon2Params{}, nil, nil, errors.New("security: 哈希串格式非法")
	}

	var version int
	if _, err := fmt.Sscanf(parts[2], "v=%d", &version); err != nil || version != argon2.Version {
		return Argon2Params{}, nil, nil, errors.New("security: 哈希串版本不支持")
	}

	var p Argon2Params
	if _, err := fmt.Sscanf(parts[3], "m=%d,t=%d,p=%d", &p.Memory, &p.Iterations, &p.Parallelism); err != nil {
		return Argon2Params{}, nil, nil, errors.New("security: 哈希串参数非法")
	}

	salt, err := base64.RawStdEncoding.DecodeString(parts[4])
	if err != nil {
		return Argon2Params{}, nil, nil, errors.New("security: 哈希串盐非法")
	}
	key, err := base64.RawStdEncoding.DecodeString(parts[5])
	if err != nil {
		return Argon2Params{}, nil, nil, errors.New("security: 哈希串摘要非法")
	}
	return p, salt, key, nil
}

// ValidatePassword 实施交互设计 6.1 节的口令规则：至少 8 位，且同时包含字母与数字。
func ValidatePassword(password string) bool {
	if len(password) < 8 || len(password) > 256 {
		return false
	}
	var hasLetter, hasDigit bool
	for _, r := range password {
		switch {
		case unicode.IsLetter(r):
			hasLetter = true
		case unicode.IsDigit(r):
			hasDigit = true
		}
	}
	return hasLetter && hasDigit
}

// PasswordStrength 返回 6.1 节的三档强度：0 弱 / 1 一般 / 2 强。
// 供官网强度条使用，服务端只用它做展示提示，不作为准入条件。
func PasswordStrength(password string) int {
	score := 0
	if len(password) >= 12 {
		score++
	}
	var hasUpper, hasSymbol bool
	for _, r := range password {
		switch {
		case unicode.IsUpper(r):
			hasUpper = true
		case !unicode.IsLetter(r) && !unicode.IsDigit(r):
			hasSymbol = true
		}
	}
	if hasUpper && hasSymbol {
		score++
	}
	return score
}
