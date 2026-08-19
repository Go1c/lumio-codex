package sub2api

import (
	"encoding/json"
	"errors"
	"testing"
)

func TestFormatYuan(t *testing.T) {
	got, err := FormatYuan(1990)
	if err != nil || got != "19.90" {
		t.Fatalf("FormatYuan(1990) = %q, %v", got, err)
	}
	if _, err := FormatYuan(0); !errors.Is(err, ErrInvalidAmount) {
		t.Fatalf("零金额应拒绝, err=%v", err)
	}
	if _, err := FormatYuan(-1); !errors.Is(err, ErrInvalidAmount) {
		t.Fatalf("负数应拒绝, err=%v", err)
	}
}

func TestParseYuanRejectsInvalidValues(t *testing.T) {
	cases := []string{"", "0", "0.00", "-19.90", "19.901", "19.", ".90", "1e2", "+19.90", "19.9.0", "abc"}
	for _, raw := range cases {
		if _, err := ParseYuan(raw); !errors.Is(err, ErrInvalidAmount) {
			t.Errorf("ParseYuan(%q) = %v, want ErrInvalidAmount", raw, err)
		}
	}
}

func TestParseYuanAcceptsTwoDecimalForms(t *testing.T) {
	got, err := ParseYuan("19.9")
	if err != nil || got != 1990 {
		t.Fatalf("19.9 → %d, %v", got, err)
	}
	got, err = ParseYuan("19.90")
	if err != nil || got != 1990 {
		t.Fatalf("19.90 → %d, %v", got, err)
	}
	got, err = ParseYuan("10.10000000")
	if err != nil || got != 1010 {
		t.Fatalf("10.10000000 → %d, %v", got, err)
	}
	got, err = ParseYuanJSON(json.RawMessage(`3.25000000`))
	if err != nil || got != 325 {
		t.Fatalf("3.25000000 → %d, %v", got, err)
	}
}

func TestParseYuanJSONRejectsStringAmount(t *testing.T) {
	if _, err := ParseYuanJSON(json.RawMessage(`"19.90"`)); !errors.Is(err, ErrInvalidAmount) {
		t.Fatalf("字符串金额应拒绝, err=%v", err)
	}
	got, err := ParseYuanJSON(json.RawMessage(`19.90`))
	if err != nil || got != 1990 {
		t.Fatalf("数字 19.90 → %d, %v", got, err)
	}
}

func TestParseYuanSnapshotAllowsZeroAndRoundsExtraDigits(t *testing.T) {
	cases := []struct {
		raw  string
		want int64
	}{
		{"0", 0},
		{"0.00000000", 0},
		{"583.46000000", 58346},
		{"583.45999999", 58346},
		{"19.90", 1990},
		{"19.9", 1990},
		{"10.10000000", 1010},
		{"9.995", 1000},
	}
	for _, tc := range cases {
		got, err := ParseYuanSnapshot(tc.raw)
		if err != nil || got != tc.want {
			t.Errorf("ParseYuanSnapshot(%q) = %d, %v, want %d", tc.raw, got, err, tc.want)
		}
	}
}

func TestParseYuanSnapshotJSONAcceptsStringAndRejectsNull(t *testing.T) {
	got, err := ParseYuanSnapshotJSON(json.RawMessage(`"583.46000000"`))
	if err != nil || got != 58346 {
		t.Fatalf("字符串快照 583.46000000 → %d, %v", got, err)
	}
	if _, err := ParseYuanSnapshotJSON(json.RawMessage(``)); !errors.Is(err, ErrInvalidAmount) {
		t.Fatalf("空字段应解析失败好打日志, err=%v", err)
	}
	if _, err := ParseYuanSnapshotJSON(json.RawMessage(`null`)); !errors.Is(err, ErrInvalidAmount) {
		t.Fatalf("null 应解析失败, err=%v", err)
	}
	if _, err := ParseYuan("19.901"); !errors.Is(err, ErrInvalidAmount) {
		t.Fatal("请求金额 19.901 仍须被严格 ParseYuan 拒绝")
	}
}
