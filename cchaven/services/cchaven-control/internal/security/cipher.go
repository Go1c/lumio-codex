package security

import (
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"errors"
	"fmt"
)

// ErrDecrypt 表示密文损坏或密钥不匹配。
var ErrDecrypt = errors.New("security: 解密失败")

// Cipher 用 AES-256-GCM 加密需要还原明文的少量机密（目前仅管理员 TOTP 种子）。
// 用户口令走 Argon2id 单向哈希，绝不进这里。
type Cipher struct{ aead cipher.AEAD }

// NewCipher 由任意长度的密钥材料派生 AES-256 密钥并构造 AEAD。
func NewCipher(keyMaterial []byte) (*Cipher, error) {
	key := sha256.Sum256(keyMaterial)

	block, err := aes.NewCipher(key[:])
	if err != nil {
		return nil, fmt.Errorf("security: 构造分组密码失败: %w", err)
	}
	aead, err := cipher.NewGCM(block)
	if err != nil {
		return nil, fmt.Errorf("security: 构造 GCM 失败: %w", err)
	}
	return &Cipher{aead: aead}, nil
}

// Encrypt 加密明文，返回 base64(nonce || ciphertext)。
func (c *Cipher) Encrypt(plaintext string) (string, error) {
	nonce := make([]byte, c.aead.NonceSize())
	if _, err := rand.Read(nonce); err != nil {
		return "", fmt.Errorf("security: 生成 nonce 失败: %w", err)
	}
	sealed := c.aead.Seal(nonce, nonce, []byte(plaintext), nil)
	return base64.RawStdEncoding.EncodeToString(sealed), nil
}

// Decrypt 还原密文。
func (c *Cipher) Decrypt(encoded string) (string, error) {
	raw, err := base64.RawStdEncoding.DecodeString(encoded)
	if err != nil {
		return "", ErrDecrypt
	}
	if len(raw) < c.aead.NonceSize() {
		return "", ErrDecrypt
	}

	nonce, ciphertext := raw[:c.aead.NonceSize()], raw[c.aead.NonceSize():]
	plaintext, err := c.aead.Open(nil, nonce, ciphertext, nil)
	if err != nil {
		return "", ErrDecrypt
	}
	return string(plaintext), nil
}
