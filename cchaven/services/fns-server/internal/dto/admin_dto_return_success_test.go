package dto

import (
	"encoding/json"
	"testing"
)

func TestAdminConfigAcceptsCorrectReturnSuccessJSON(t *testing.T) {
	var cfg AdminConfig
	if err := json.Unmarshal([]byte(`{"isReturnSuccess":true}`), &cfg); err != nil {
		t.Fatalf("Unmarshal: %v", err)
	}
	got := cfg.ReturnSuccessFlag()
	if got == nil || !*got {
		t.Fatalf("isReturnSuccess:true did not populate ReturnSuccessFlag, got %#v (legacy-only isReturnSussess?)", got)
	}
}

func TestAdminConfigStillAcceptsMisspelledSussessJSON(t *testing.T) {
	var cfg AdminConfig
	if err := json.Unmarshal([]byte(`{"isReturnSussess":true}`), &cfg); err != nil {
		t.Fatalf("Unmarshal: %v", err)
	}
	got := cfg.ReturnSuccessFlag()
	if got == nil || !*got {
		t.Fatalf("legacy isReturnSussess:true should remain accepted")
	}
}
