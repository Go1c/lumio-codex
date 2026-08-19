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
