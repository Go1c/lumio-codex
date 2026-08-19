package store

import "testing"

func TestDefaultMonthlyPriceIs1990(t *testing.T) {
	if defaultConfig.PricingMonthly.AmountCents != 1990 {
		t.Fatalf("缺省包月价 = %d 分, want 1990", defaultConfig.PricingMonthly.AmountCents)
	}
	if defaultConfig.PricingMonthly.Currency != "CNY" {
		t.Errorf("缺省币种 = %q, want CNY", defaultConfig.PricingMonthly.Currency)
	}
}
