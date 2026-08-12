package service

import "testing"

// TestRoleCapabilityMatrix 逐格锁定权限矩阵。
//
// 这张表就是 docs/m1-spec.md 2.8 的那张表，改动其一必须同时改另一处。
// 核心不变量：破坏性操作的门槛不得低于读取敏感信息——support 一格都没有。
func TestRoleCapabilityMatrix(t *testing.T) {
	all := []AdminCapability{
		CapViewUserDetail, CapManageUsers, CapRefundOrder, CapEditOpsConfig, CapExportOrders,
	}

	cases := []struct {
		role string
		want bool
	}{
		{RoleOwner, true},
		{RoleOps, true},
		{RoleSupport, false},
	}
	for _, tc := range cases {
		t.Run(tc.role, func(t *testing.T) {
			for _, capability := range all {
				if got := Can(tc.role, capability); got != tc.want {
					t.Errorf("Can(%q, %q) = %v, want %v", tc.role, capability, got, tc.want)
				}
			}
		})
	}
}

// TestUnknownRoleAndCapabilityAreDenied 验证默认拒绝：
// 数据库里出现未知角色、或代码里问了一个没登记的能力，都不能意外放行。
func TestUnknownRoleAndCapabilityAreDenied(t *testing.T) {
	if Can("intern", CapManageUsers) {
		t.Error("未知角色不应具备任何能力")
	}
	if Can("", CapViewUserDetail) {
		t.Error("空角色不应具备任何能力")
	}
	if Can(RoleOwner, AdminCapability("delete_everything")) {
		t.Error("未登记的能力不应被放行")
	}
}

// TestCapabilityPredicatesMatchMatrix 保证语义化谓词与矩阵不会各说各话。
func TestCapabilityPredicatesMatchMatrix(t *testing.T) {
	predicates := map[AdminCapability]func(string) bool{
		CapViewUserDetail: CanViewUserDetail,
		CapManageUsers:    CanManageUsers,
		CapRefundOrder:    CanRefundOrder,
		CapEditOpsConfig:  CanEditOpsConfig,
		CapExportOrders:   CanExportOrders,
	}
	for capability, predicate := range predicates {
		for _, role := range []string{RoleOwner, RoleOps, RoleSupport, "intern"} {
			if got, want := predicate(role), Can(role, capability); got != want {
				t.Errorf("%q 的谓词对 %q 返回 %v, 矩阵为 %v", capability, role, got, want)
			}
		}
	}
}

// TestDisableActionNames 锁定审计动作名，`_denied` 后缀由 auditDenied 追加。
func TestDisableActionNames(t *testing.T) {
	if got := disableAction(true); got != "user.disable" {
		t.Errorf("禁用动作 = %q", got)
	}
	if got := disableAction(false); got != "user.enable" {
		t.Errorf("解禁动作 = %q", got)
	}
}
