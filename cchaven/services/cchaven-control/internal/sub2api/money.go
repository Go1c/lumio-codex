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
		if frac == "" || len(frac) > 2 || !allDigits(frac) {
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
