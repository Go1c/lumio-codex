package i18n

import "testing"

// TestSection62CopyIsVerbatim 锁定交互设计 6.2 节的七条固定文案。
// 这些文案承载防枚举与限频的安全语义，任何改动都必须先改规范、再改这里。
func TestSection62CopyIsVerbatim(t *testing.T) {
	cases := []struct {
		name string
		id   ID
		args map[string]string
		want string
	}{
		{
			name: "登录失败",
			id:   MsgLoginFailed,
			want: "邮箱或密码不正确。",
		},
		{
			name: "忘记密码提交后",
			id:   MsgForgotPasswordSubmitted,
			args: map[string]string{"email": "mary@example.com"},
			want: "如 mary@example.com 已注册账号，你将很快收到重设链接。",
		},
		{
			name: "验证码错误",
			id:   MsgCodeInvalid,
			args: map[string]string{"n": "3"},
			want: "验证码不正确，还剩 3 次尝试机会。",
		},
		{
			name: "验证码过期",
			id:   MsgCodeExpired,
			want: "该验证码已过期，请重新发送。",
		},
		{
			name: "限频（分钟）",
			id:   MsgRateLimited,
			args: map[string]string{"n": "1", "unit": "分钟"},
			want: "尝试次数过多，请 1 分钟后再试。",
		},
		{
			name: "限频（秒）",
			id:   MsgRateLimited,
			args: map[string]string{"n": "42", "unit": "秒"},
			want: "尝试次数过多，请 42 秒后再试。",
		},
		{
			name: "会话过期",
			id:   MsgSessionExpired,
			want: "登录已过期，请重新登录。",
		},
		{
			name: "试用重复领取",
			id:   MsgTrialAlreadyUsed,
			want: "每个账号只可享用一次免费试用。",
		},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := T(ZhCN, tc.id, tc.args); got != tc.want {
				t.Errorf("文案不符\n got: %q\nwant: %q", got, tc.want)
			}
		})
	}
}

// TestZhHKFallsBackToZhCN 确认繁体中文未补词条时不会返回空串或条目 ID。
func TestZhHKFallsBackToZhCN(t *testing.T) {
	if got, want := T(ZhHK, MsgLoginFailed, nil), "邮箱或密码不正确。"; got != want {
		t.Errorf("zh-HK 未回落到 zh-CN: got %q want %q", got, want)
	}
}

// TestEveryIDHasZhCNCopy 防止新增条目忘记补简体中文文案。
func TestEveryIDHasZhCNCopy(t *testing.T) {
	ids := []ID{
		MsgLoginFailed, MsgForgotPasswordSubmitted, MsgCodeInvalid, MsgCodeExpired,
		MsgRateLimited, MsgSessionExpired, MsgTrialAlreadyUsed,
		MsgEmailTaken, MsgEmailInvalid, MsgEmailUnverified, MsgAccountDisabled,
		MsgResetLinkInvalid, MsgPasswordTooWeak, MsgPasswordUpdatedAll, MsgPasswordUpdatedSelf,
		MsgCurrentPasswordBad, MsgPasswordMismatch, MsgAuthMigrated, MsgIdentityUnavailable,
		MsgUnauthorized, MsgForbidden, MsgNotFound,
		MsgInvalidParams, MsgInternal, MsgInviteCodeInvalid, MsgInviteSelf,
		MsgOAuthInvalidRequest, MsgOAuthInvalidGrant, MsgAdminMFARequired, MsgAdminMFAInvalid,
		MsgOrderNotRefundable, MsgInsufficientBalance, MsgDebitUnavailable,
	}
	for _, id := range ids {
		if _, ok := dictionaries[ZhCN][id]; !ok {
			t.Errorf("条目 %s 缺少 zh-CN 文案", id)
		}
	}
}
