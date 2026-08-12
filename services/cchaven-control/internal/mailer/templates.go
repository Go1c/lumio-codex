package mailer

import (
	"fmt"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/store"
)

// Render 把发件箱条目渲染为邮件标题与正文。
//
// 文案与界面保持同一套用语（「CC避风港」而非 CCHaven 单用），
// 未知模板降级为通用通知而不是丢件。
func Render(template string, payload map[string]any) (subject, body string) {
	switch template {
	case store.TemplateVerifyCode:
		return "CC避风港 邮箱验证码", fmt.Sprintf(
			"你的验证码是 %s，%v 分钟内有效。\n\n如果这不是你本人的操作，请忽略本邮件。",
			str(payload, "code"), num(payload, "expires_in"))

	case store.TemplateEmailChange:
		return "CC避风港 邮箱变更验证码", fmt.Sprintf(
			"你正在把 CC避风港 账号的邮箱改为本邮箱，验证码是 %s，%v 分钟内有效。\n\n"+
				"如果这不是你本人的操作，请忽略本邮件。",
			str(payload, "code"), num(payload, "expires_in"))

	case store.TemplateEmailChanged:
		return "CC避风港 账号邮箱已变更", fmt.Sprintf(
			"你的 CC避风港 账号邮箱已变更为 %s。\n\n如果这不是你本人的操作，请立即联系我们。",
			str(payload, "new_email"))

	case store.TemplatePasswordReset:
		return "CC避风港 重设密码", fmt.Sprintf(
			"点击以下链接重设密码，链接 %v 分钟内有效且只能使用一次：\n\n%s\n\n"+
				"如果这不是你本人的操作，请忽略本邮件，你的密码不会被更改。",
			num(payload, "expires_in"), str(payload, "reset_url"))

	case store.TemplateTrialGranted:
		return "🎁 你的 CC避风港 免费试用已开通", fmt.Sprintf(
			"免费试用已开通，共 %v 天，有效期至 %s。\n\n现在就打开 CC避风港，开始使用吧。",
			num(payload, "days"), str(payload, "expires_at"))

	case store.TemplateInviteRewarded:
		return "你邀请的朋友已加入 CC避风港", fmt.Sprintf(
			"你邀请的 %s 已完成注册并登录 APP，你的订阅已延长 %v 天。",
			str(payload, "friend"), num(payload, "days"))

	case store.TemplateDeletionNotice:
		return "CC避风港 账号注销申请", fmt.Sprintf(
			"我们已收到你的账号注销申请，将于 %s 生效。\n\n"+
				"在此之前你可以随时在账户中心撤销注销。",
			str(payload, "effective_at"))

	default:
		return "CC避风港 通知", "你有一条来自 CC避风港 的通知。"
	}
}

func str(payload map[string]any, key string) string {
	if v, ok := payload[key].(string); ok {
		return v
	}
	return ""
}

func num(payload map[string]any, key string) any {
	if v, ok := payload[key]; ok {
		// JSON 数字解出来是 float64，去掉小数点以免出现「10.000000 分钟」。
		if f, ok := v.(float64); ok {
			return int64(f)
		}
		return v
	}
	return 0
}
