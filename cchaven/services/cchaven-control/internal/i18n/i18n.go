// Package i18n 集中管理服务端下发的界面文案。
//
// 交互设计 6.2 节的防枚举与限频文案属于安全语义，必须逐字使用；本包是它们的唯一出处，
// 并由 i18n_test.go 逐条锁定。6.5 节要求预留繁体中文（香港），故字典按语言分层，
// 缺失条目回落到 zh-CN。
package i18n

import "strings"

// Lang 是界面语言标识。
type Lang string

const (
	// ZhCN 简体中文，默认语言。
	ZhCN Lang = "zh-CN"
	// ZhHK 繁体中文（香港），词条待补，目前整体回落 zh-CN。
	ZhHK Lang = "zh-HK"
)

// ID 是文案条目标识。
type ID string

// 交互设计 6.2 节「防枚举与限频文案」——不得改写。
const (
	// MsgLoginFailed 登录失败（不区分邮箱或密码，防枚举）。
	MsgLoginFailed ID = "auth.login_failed"
	// MsgForgotPasswordSubmitted 忘记密码提交后的恒定回执，无论邮箱是否存在。
	MsgForgotPasswordSubmitted ID = "auth.forgot_password_submitted"
	// MsgCodeInvalid 验证码错误，带剩余次数。
	MsgCodeInvalid ID = "auth.code_invalid"
	// MsgCodeExpired 验证码过期。
	MsgCodeExpired ID = "auth.code_expired"
	// MsgRateLimited 限频，带数值与单位。
	MsgRateLimited ID = "common.rate_limited"
	// MsgSessionExpired 会话过期。
	MsgSessionExpired ID = "auth.session_expired"
	// MsgTrialAlreadyUsed 试用重复领取。
	MsgTrialAlreadyUsed ID = "billing.trial_already_used"
)

// 其余来自交互设计正文与原型的固定文案。
const (
	MsgEmailTaken          ID = "auth.email_taken"           // 4.5 注册页
	MsgEmailInvalid        ID = "auth.email_invalid"         // 4.5 注册页
	MsgEmailUnverified     ID = "auth.email_unverified"      // 4.7 登录页
	MsgAccountDisabled     ID = "auth.account_disabled"      // 3.2 登录状态机
	MsgResetLinkInvalid    ID = "auth.reset_link_invalid"    // 4.8 重设密码页
	MsgPasswordTooWeak     ID = "auth.password_too_weak"     // 6.1 密码规则
	MsgPasswordUpdatedAll  ID = "auth.password_updated_all"  // 4.8 重设成功
	MsgPasswordUpdatedSelf ID = "auth.password_updated_self" // 5.6 修改密码成功
	MsgCurrentPasswordBad  ID = "auth.current_password_bad"  // 5.6 修改密码失败
	MsgPasswordMismatch    ID = "auth.password_mismatch"     // 原型 重设密码页
	MsgAuthMigrated        ID = "auth.migrated"              // 身份收口到 Sub2API 后的下线回执
	MsgIdentityUnavailable ID = "auth.identity_unavailable"  // Sub2API 不可达时的降级回执
	MsgUnauthorized        ID = "common.unauthorized"
	MsgForbidden           ID = "common.forbidden"
	MsgNotFound            ID = "common.not_found"
	MsgInvalidParams       ID = "common.invalid_params"
	MsgInternal            ID = "common.internal"
	MsgInviteCodeInvalid   ID = "invite.code_invalid" // 4.4 邀请落地页
	MsgInviteSelf          ID = "invite.self"
	MsgOAuthInvalidRequest ID = "oauth.invalid_request"
	MsgOAuthInvalidGrant   ID = "oauth.invalid_grant"
	MsgAdminMFARequired    ID = "admin.mfa_required"
	MsgAdminMFAInvalid     ID = "admin.mfa_invalid"
	MsgOrderNotRefundable  ID = "admin.order_not_refundable"
	MsgRefundDeclined      ID = "admin.refund_declined" // 渠道拒绝退款（QA S-9）
)

var dictionaries = map[Lang]map[ID]string{
	ZhCN: {
		// —— 6.2 节固定文案，逐字对照，不得改写 ——
		MsgLoginFailed:             "邮箱或密码不正确。",
		MsgForgotPasswordSubmitted: "如 {email} 已注册账号，你将很快收到重设链接。",
		MsgCodeInvalid:             "验证码不正确，还剩 {n} 次尝试机会。",
		MsgCodeExpired:             "该验证码已过期，请重新发送。",
		MsgRateLimited:             "尝试次数过多，请 {n} {unit}后再试。",
		MsgSessionExpired:          "登录已过期，请重新登录。",
		MsgTrialAlreadyUsed:        "每个账号只可享用一次免费试用。",

		// —— 正文与原型文案 ——
		MsgEmailTaken:          "该邮箱已注册。",
		MsgEmailInvalid:        "请输入有效的邮箱地址。",
		MsgEmailUnverified:     "你的邮箱尚未验证。",
		MsgAccountDisabled:     "账号已停用，请联系支持。",
		MsgResetLinkInvalid:    "该链接已过期或已被使用。",
		MsgPasswordTooWeak:     "密码至少 8 位，且需同时包含字母和数字。",
		MsgPasswordUpdatedAll:  "密码已更新，所有设备已退出登录。",
		MsgPasswordUpdatedSelf: "密码已更新，其他设备已退出登录。",
		MsgCurrentPasswordBad:  "当前密码不正确。",
		MsgPasswordMismatch:    "两次输入的密码不一致。",
		MsgAuthMigrated:        "账号体系已统一到 Lumio 账号中心，请前往 {portal} 登录。",
		MsgIdentityUnavailable: "账号服务暂时不可用，请稍后重试。",
		MsgUnauthorized:        "请先登录。",
		MsgForbidden:           "没有访问权限。",
		MsgNotFound:            "请求的资源不存在。",
		MsgInvalidParams:       "请求参数不正确。",
		MsgInternal:            "服务暂时不可用，请稍后重试。",
		MsgInviteCodeInvalid:   "此邀请链接已失效。",
		MsgInviteSelf:          "不能使用自己的邀请链接。",
		MsgOAuthInvalidRequest: "授权请求参数不正确。",
		MsgOAuthInvalidGrant:   "授权码无效或已过期，请重新授权。",
		MsgAdminMFARequired:    "请输入两步验证码。",
		MsgAdminMFAInvalid:     "两步验证码不正确。",
		MsgOrderNotRefundable:  "该订单当前状态不支持退款。",
		MsgRefundDeclined:      "退款被支付渠道拒绝，订单已恢复为已支付，可排查后重试。",
	},
	// zh-HK 词条待补；缺失时自动回落 zh-CN。
	ZhHK: {},
}

// T 渲染指定语言的文案，args 中的键以 {key} 形式插值。
// 未知语言或缺失条目回落到 zh-CN；仍缺失则返回条目 ID 本身，便于在测试中暴露遗漏。
func T(lang Lang, id ID, args map[string]string) string {
	tpl, ok := dictionaries[lang][id]
	if !ok {
		if tpl, ok = dictionaries[ZhCN][id]; !ok {
			return string(id)
		}
	}
	if len(args) == 0 {
		return tpl
	}

	pairs := make([]string, 0, len(args)*2)
	for k, v := range args {
		pairs = append(pairs, "{"+k+"}", v)
	}
	return strings.NewReplacer(pairs...).Replace(tpl)
}
