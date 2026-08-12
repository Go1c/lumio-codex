package security

import (
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"math/big"
	"strings"
)

// RandomToken 生成 n 字节随机数并以 URL-safe base64 编码。
// 重设密码链接按规范使用 32 字节。
func RandomToken(n int) (string, error) {
	buf := make([]byte, n)
	if _, err := rand.Read(buf); err != nil {
		return "", fmt.Errorf("security: 生成随机令牌失败: %w", err)
	}
	return base64.RawURLEncoding.EncodeToString(buf), nil
}

// HashToken 返回令牌的 SHA-256 摘要（十六进制）。
// 数据库只存摘要，明文令牌仅存在于响应体与邮件中。
func HashToken(token string) string {
	sum := sha256.Sum256([]byte(token))
	return hex.EncodeToString(sum[:])
}

// HashCode 用带 pepper 的 HMAC-SHA256 摘要验证码。
// 6 位数字空间很小，加 pepper 可防止数据库泄露后被离线枚举。
func HashCode(code string, pepper []byte) string {
	mac := hmac.New(sha256.New, pepper)
	mac.Write([]byte(code))
	return hex.EncodeToString(mac.Sum(nil))
}

// NumericCode 生成 n 位十进制验证码，允许前导零。
func NumericCode(n int) (string, error) {
	var sb strings.Builder
	sb.Grow(n)
	for range n {
		digit, err := rand.Int(rand.Reader, big.NewInt(10))
		if err != nil {
			return "", fmt.Errorf("security: 生成验证码失败: %w", err)
		}
		sb.WriteByte(byte('0' + digit.Int64()))
	}
	return sb.String(), nil
}

// referralAlphabet 剔除了 0/o/1/l/i 等易混字符，便于用户口头传播邀请码。
const referralAlphabet = "abcdefghjkmnpqrstuvwxyz23456789"

// ReferralCode 生成 n 位邀请码（原型示例 mary8k2f 为 8 位）。
func ReferralCode(n int) (string, error) {
	var sb strings.Builder
	sb.Grow(n)
	max := big.NewInt(int64(len(referralAlphabet)))
	for range n {
		idx, err := rand.Int(rand.Reader, max)
		if err != nil {
			return "", fmt.Errorf("security: 生成邀请码失败: %w", err)
		}
		sb.WriteByte(referralAlphabet[idx.Int64()])
	}
	return sb.String(), nil
}

// VerifyPKCE 校验 RFC 7636 的 S256 code challenge。
func VerifyPKCE(codeVerifier, codeChallenge string) bool {
	// verifier 长度由 RFC 7636 4.1 规定为 43–128 个字符。
	if len(codeVerifier) < 43 || len(codeVerifier) > 128 {
		return false
	}
	sum := sha256.Sum256([]byte(codeVerifier))
	computed := base64.RawURLEncoding.EncodeToString(sum[:])
	return hmac.Equal([]byte(computed), []byte(codeChallenge))
}
