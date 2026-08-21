package config

import (
	"testing"

	"gopkg.in/yaml.v3"
)

func TestAppSettingsAcceptsCorrectSuccessSpelling(t *testing.T) {
	var settings AppSettings
	if err := yaml.Unmarshal([]byte("is-return-success: true\n"), &settings); err != nil {
		t.Fatalf("Unmarshal correct spelling: %v", err)
	}
	if !settings.ReturnSuccessEnabled() {
		t.Fatalf("is-return-success: true did not enable ReturnSuccess (typo-only tag still in effect?)")
	}
}

func TestAppSettingsStillAcceptsMisspelledSussess(t *testing.T) {
	var settings AppSettings
	if err := yaml.Unmarshal([]byte("is-return-sussess: true\n"), &settings); err != nil {
		t.Fatalf("Unmarshal misspelling alias: %v", err)
	}
	if !settings.ReturnSuccessEnabled() {
		t.Fatalf("legacy is-return-sussess: true should remain accepted")
	}
}
