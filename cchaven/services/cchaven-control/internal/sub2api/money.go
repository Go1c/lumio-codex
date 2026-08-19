package sub2api

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"strconv"
	"strings"
	"unicode"
)

// ErrInvalidAmount 表示金额不是合法的两位小数人民币额度。
var ErrInvalidAmount = errors.New("sub2api: 金额不合法")

// FormatYuan 把分格式化成 JSON 数字字面量（例如 1990 → 19.90），不经过 float。
func FormatYuan(cents int64) (string, error) {
	if cents <= 0 {
		return "", ErrInvalidAmount
	}
	return fmt.Sprintf("%d.%02d", cents/100, cents%100), nil
}

// ParseYuan 从十进制文本解析元→分。拒绝零、负数、科学计数、超过两位小数。
func ParseYuan(raw string) (int64, error) {
	raw = strings.TrimSpace(raw)
	if raw == "" || strings.ContainsAny(raw, "eE+") || strings.HasPrefix(raw, "-") {
		return 0, ErrInvalidAmount
	}
	whole, frac, dotted := strings.Cut(raw, ".")
	if whole == "" || !allDigits(whole) {
		return 0, ErrInvalidAmount
	}
	if dotted {
		if frac == "" || !allDigits(frac) {
			return 0, ErrInvalidAmount
		}
		// LumioAPI wallet balance is numeric(20,8) and comes back as 10.10000000.
		// Trailing zeros are the same amount; extra non-zero digits are not cents.
		frac = strings.TrimRight(frac, "0")
		if frac == "" {
			dotted = false
		} else if len(frac) > 2 {
			return 0, ErrInvalidAmount
		}
	}
	yuan, err := strconv.ParseInt(whole, 10, 64)
	if err != nil {
		return 0, ErrInvalidAmount
	}
	var fraction int64
	if dotted {
		fraction, err = strconv.ParseInt(frac, 10, 64)
		if err != nil {
			return 0, ErrInvalidAmount
		}
		if len(frac) == 1 {
			fraction *= 10
		}
	}
	cents := yuan*100 + fraction
	if cents <= 0 {
		return 0, ErrInvalidAmount
	}
	return cents, nil
}

// ParseYuanJSON 只接受 JSON 数字。字符串金额一律拒绝。
func ParseYuanJSON(raw json.RawMessage) (int64, error) {
	trimmed := bytes.TrimSpace(raw)
	if len(trimmed) == 0 || trimmed[0] == '"' {
		return 0, ErrInvalidAmount
	}
	return ParseYuan(string(trimmed))
}

// ParseYuanSnapshot 解析回执上的余额快照。允许 0；超过两位小数按分四舍五入。
// 请求金额仍走 ParseYuan，避免 19.901 被悄悄收下。
func ParseYuanSnapshot(raw string) (int64, error) {
	raw = strings.TrimSpace(raw)
	if raw == "" || strings.ContainsAny(raw, "eE+") || strings.HasPrefix(raw, "-") {
		return 0, ErrInvalidAmount
	}
	whole, frac, dotted := strings.Cut(raw, ".")
	if whole == "" || !allDigits(whole) {
		return 0, ErrInvalidAmount
	}
	yuan, err := strconv.ParseInt(whole, 10, 64)
	if err != nil {
		return 0, ErrInvalidAmount
	}
	if !dotted {
		return yuan * 100, nil
	}
	if frac == "" || !allDigits(frac) {
		return 0, ErrInvalidAmount
	}
	for len(frac) < 2 {
		frac += "0"
	}
	fraction, err := strconv.ParseInt(frac[:2], 10, 64)
	if err != nil {
		return 0, ErrInvalidAmount
	}
	cents := yuan*100 + fraction
	if len(frac) > 2 && frac[2] >= '5' {
		cents++
	}
	return cents, nil
}

// ParseYuanSnapshotJSON 解析回执余额。数字或十进制字符串均可；空 / null 失败以便打原文。
func ParseYuanSnapshotJSON(raw json.RawMessage) (int64, error) {
	trimmed := bytes.TrimSpace(raw)
	if len(trimmed) == 0 || string(trimmed) == "null" {
		return 0, ErrInvalidAmount
	}
	if trimmed[0] == '"' {
		var text string
		if err := json.Unmarshal(trimmed, &text); err != nil {
			return 0, ErrInvalidAmount
		}
		return ParseYuanSnapshot(text)
	}
	return ParseYuanSnapshot(string(trimmed))
}

func allDigits(value string) bool {
	if value == "" {
		return false
	}
	for _, r := range value {
		if !unicode.IsDigit(r) {
			return false
		}
	}
	return true
}
