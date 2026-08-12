package security

import (
	"crypto/sha256"
	"encoding/base64"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
)

func TestArgon2HashRoundTrip(t *testing.T) {
	hasher := NewHasher(TestArgon2Params())

	hash, err := hasher.Hash("Passw0rd!")
	if err != nil {
		t.Fatalf("Hash 失败: %v", err)
	}
	if !strings.HasPrefix(hash, "$argon2id$v=19$") {
		t.Errorf("哈希串格式不符 PHC 规范: %s", hash)
	}
	// 口令绝不能以任何形式出现在哈希串里。
	if strings.Contains(hash, "Passw0rd") {
		t.Error("哈希串中不应包含明文口令")
	}

	if err := hasher.Verify("Passw0rd!", hash); err != nil {
		t.Errorf("正确口令校验失败: %v", err)
	}
	if err := hasher.Verify("wrong-password1", hash); !errors.Is(err, ErrPasswordMismatch) {
		t.Errorf("错误口令应返回 ErrPasswordMismatch, got %v", err)
	}
}

func TestArgon2SaltIsRandom(t *testing.T) {
	hasher := NewHasher(TestArgon2Params())

	first, _ := hasher.Hash("Passw0rd!")
	second, _ := hasher.Hash("Passw0rd!")
	if first == second {
		t.Error("相同口令两次哈希应因随机盐而不同")
	}
}

func TestVerifyRejectsMalformedHash(t *testing.T) {
	hasher := NewHasher(TestArgon2Params())

	for _, bad := range []string{"", "plain-text", "$argon2i$v=19$m=8,t=1,p=1$c2FsdA$aGFzaA"} {
		if err := hasher.Verify("Passw0rd!", bad); err == nil {
			t.Errorf("非法哈希串 %q 应校验失败", bad)
		}
	}
}

func TestValidatePassword(t *testing.T) {
	cases := []struct {
		password string
		want     bool
	}{
		{"Passw0rd", true},
		{"abcd1234", true},
		{"短1", false},                      // 少于 8 位
		{"abcdefgh", false},                // 无数字
		{"12345678", false},                // 无字母
		{"", false},                        // 空
		{strings.Repeat("a1", 200), false}, // 超长
	}

	for _, tc := range cases {
		if got := ValidatePassword(tc.password); got != tc.want {
			t.Errorf("ValidatePassword(%q) = %v, want %v", tc.password, got, tc.want)
		}
	}
}

func TestNumericCode(t *testing.T) {
	code, err := NumericCode(6)
	if err != nil {
		t.Fatalf("NumericCode 失败: %v", err)
	}
	if len(code) != 6 {
		t.Errorf("验证码长度 = %d, want 6", len(code))
	}
	for _, r := range code {
		if r < '0' || r > '9' {
			t.Errorf("验证码含非数字字符: %q", code)
		}
	}
}

func TestHashCodeUsesPepper(t *testing.T) {
	if HashCode("123456", []byte("pepper-a")) == HashCode("123456", []byte("pepper-b")) {
		t.Error("不同 pepper 应产生不同摘要")
	}
	if HashCode("123456", []byte("pepper")) != HashCode("123456", []byte("pepper")) {
		t.Error("同一输入应产生稳定摘要")
	}
}

func TestReferralCodeAlphabet(t *testing.T) {
	code, err := ReferralCode(8)
	if err != nil {
		t.Fatalf("ReferralCode 失败: %v", err)
	}
	if len(code) != 8 {
		t.Errorf("邀请码长度 = %d, want 8", len(code))
	}
	// 邀请码可能被口头传播，必须剔除易混字符。
	if strings.ContainsAny(code, "01loi") {
		t.Errorf("邀请码不应包含易混字符: %q", code)
	}
}

func TestVerifyPKCE(t *testing.T) {
	verifier := strings.Repeat("a", 64)
	sum := sha256.Sum256([]byte(verifier))
	challenge := base64.RawURLEncoding.EncodeToString(sum[:])

	if !VerifyPKCE(verifier, challenge) {
		t.Error("正确的 verifier 应通过校验")
	}
	if VerifyPKCE(strings.Repeat("b", 64), challenge) {
		t.Error("错误的 verifier 不应通过校验")
	}
	// RFC 7636 规定 verifier 长度为 43–128。
	if VerifyPKCE("too-short", challenge) {
		t.Error("过短的 verifier 应被拒绝")
	}
}

func TestTokenIssuerRoundTrip(t *testing.T) {
	issuer := NewTokenIssuer([]byte("test-secret-key-at-least-32-bytes!!"), 15*time.Minute)
	sessionID := uuid.New()
	now := time.Now()

	token, err := issuer.Issue(100986, sessionID, "profile workspace", now)
	if err != nil {
		t.Fatalf("Issue 失败: %v", err)
	}

	claims, err := issuer.Parse(token)
	if err != nil {
		t.Fatalf("Parse 失败: %v", err)
	}
	if claims.UserID != 100986 {
		t.Errorf("UserID = %d, want 100986", claims.UserID)
	}
	if claims.SessionID != sessionID {
		t.Errorf("SessionID = %v, want %v", claims.SessionID, sessionID)
	}
	if claims.Scope != "profile workspace" {
		t.Errorf("Scope = %q", claims.Scope)
	}
}

func TestTokenIssuerRejectsTamperedAndExpired(t *testing.T) {
	secret := []byte("test-secret-key-at-least-32-bytes!!")
	issuer := NewTokenIssuer(secret, 15*time.Minute)
	token, _ := issuer.Issue(1, uuid.New(), "profile", time.Now())

	t.Run("换密钥无法验签", func(t *testing.T) {
		other := NewTokenIssuer([]byte("another-secret-key-at-least-32-b!!"), 15*time.Minute)
		if _, err := other.Parse(token); !errors.Is(err, ErrInvalidToken) {
			t.Errorf("应拒绝，got %v", err)
		}
	})

	t.Run("篡改载荷无法验签", func(t *testing.T) {
		if _, err := issuer.Parse(token + "x"); !errors.Is(err, ErrInvalidToken) {
			t.Errorf("应拒绝，got %v", err)
		}
	})

	t.Run("过期令牌被拒", func(t *testing.T) {
		expired, _ := issuer.Issue(1, uuid.New(), "profile", time.Now().Add(-time.Hour))
		if _, err := issuer.Parse(expired); !errors.Is(err, ErrInvalidToken) {
			t.Errorf("应拒绝，got %v", err)
		}
	})
}

func TestCipherRoundTrip(t *testing.T) {
	cipher, err := NewCipher([]byte("totp-key"))
	if err != nil {
		t.Fatalf("NewCipher 失败: %v", err)
	}

	encrypted, err := cipher.Encrypt("JBSWY3DPEHPK3PXP")
	if err != nil {
		t.Fatalf("Encrypt 失败: %v", err)
	}
	if strings.Contains(encrypted, "JBSWY3DPEHPK3PXP") {
		t.Error("密文中不应出现明文")
	}

	decrypted, err := cipher.Decrypt(encrypted)
	if err != nil {
		t.Fatalf("Decrypt 失败: %v", err)
	}
	if decrypted != "JBSWY3DPEHPK3PXP" {
		t.Errorf("解密结果 = %q", decrypted)
	}

	other, _ := NewCipher([]byte("different-key"))
	if _, err := other.Decrypt(encrypted); !errors.Is(err, ErrDecrypt) {
		t.Errorf("换密钥解密应失败，got %v", err)
	}
}
