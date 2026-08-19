// Package apperr 定义面向 API 的错误类型。
//
// 每个错误同时携带三样东西：HTTP 状态码、稳定的机器可读 code（前端据此分支）、
// 以及来自 i18n 字典的展示文案（前端可直接展示）。业务代码只构造本包的错误，
// 由 httpx 统一渲染，避免文案散落各处。
package apperr

import (
	"errors"
	"fmt"
	"net/http"
	"strconv"
	"time"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/i18n"
)

// Error 是可直接渲染为 HTTP 响应的业务错误。
type Error struct {
	Status  int               // HTTP 状态码
	Code    string            // 机器可读错误码
	Message i18n.ID           // 文案条目
	Args    map[string]string // 文案插值参数
	Details map[string]any    // 附加结构化信息（如 retry_after）
	cause   error             // 内部原因，只记日志，不出响应
}

func (e *Error) Error() string {
	if e.cause != nil {
		return fmt.Sprintf("%s: %v", e.Code, e.cause)
	}
	return e.Code
}

// Unwrap 暴露内部原因供 errors.Is/As 使用。
func (e *Error) Unwrap() error { return e.cause }

// WithCause 附加内部错误原因，仅用于日志，不会出现在响应体中。
func (e *Error) WithCause(err error) *Error {
	clone := *e
	clone.cause = err
	return &clone
}

// WithDetail 附加一项结构化细节。
func (e *Error) WithDetail(key string, value any) *Error {
	clone := *e
	clone.Details = map[string]any{}
	for k, v := range e.Details {
		clone.Details[k] = v
	}
	clone.Details[key] = value
	return &clone
}

// From 把任意 error 归一化为 *Error；非本包错误一律折叠为 500，避免内部细节外泄。
func From(err error) *Error {
	var appErr *Error
	if errors.As(err, &appErr) {
		return appErr
	}
	return Internal().WithCause(err)
}

func newError(status int, code string, msg i18n.ID) *Error {
	return &Error{Status: status, Code: code, Message: msg}
}

// —— 通用 ——

// Internal 表示未预期的服务端错误。
func Internal() *Error {
	return newError(http.StatusInternalServerError, "internal_error", i18n.MsgInternal)
}

// InvalidParams 表示请求参数不合法。
func InvalidParams() *Error {
	return newError(http.StatusBadRequest, "invalid_params", i18n.MsgInvalidParams)
}

// PaymentAmountMismatch 表示支付回调金额与订单不符。
//
// 签名合法不等于金额合法：改价、篡改、渠道错单都会触发。订单停在 pending，
// 等待真实回调或人工核对，绝不入账（QA S-5）。
func PaymentAmountMismatch() *Error {
	return newError(http.StatusBadRequest, "payment_amount_mismatch", i18n.MsgInvalidParams)
}

// RefundDeclined 表示支付渠道拒绝了退款。
//
// 退款单标 failed、订单恢复为已支付（可重试）；绝不停在 refunding——
// 那个状态没有任何推进路径（QA S-9）。
func RefundDeclined() *Error {
	return newError(http.StatusBadGateway, "refund_declined", i18n.MsgRefundDeclined)
}

// Unauthorized 表示缺少有效会话。
func Unauthorized() *Error {
	return newError(http.StatusUnauthorized, "unauthorized", i18n.MsgUnauthorized)
}

// SessionExpired 表示会话已过期或被撤销。
func SessionExpired() *Error {
	return newError(http.StatusUnauthorized, "session_expired", i18n.MsgSessionExpired)
}

// Forbidden 表示无权访问。
func Forbidden() *Error {
	return newError(http.StatusForbidden, "forbidden", i18n.MsgForbidden)
}

// NotFound 表示资源不存在。
func NotFound() *Error {
	return newError(http.StatusNotFound, "not_found", i18n.MsgNotFound)
}

// RateLimited 渲染 6.2 节限频文案，并在 details 中给出可重试时间。
// 不足 60 秒用「秒」，否则向上取整为「分钟」，与 4.5「请一分钟后再试」的视觉一致。
func RateLimited(retryAfter time.Duration) *Error {
	seconds := int(retryAfter.Seconds())
	if seconds < 1 {
		seconds = 1
	}

	n, unit := seconds, "秒"
	if seconds >= 60 {
		n, unit = (seconds+59)/60, "分钟"
	}

	err := newError(http.StatusTooManyRequests, "rate_limited", i18n.MsgRateLimited)
	err.Args = map[string]string{"n": strconv.Itoa(n), "unit": unit}
	err.Details = map[string]any{"retry_after_seconds": seconds}
	return err
}

// —— 账号与认证 ——

// EmailTaken 表示邮箱已被注册。仅用于注册接口；登录接口一律返回 InvalidCredentials 以防枚举。
func EmailTaken() *Error {
	return newError(http.StatusConflict, "email_taken", i18n.MsgEmailTaken)
}

// EmailInvalid 表示邮箱格式非法。
func EmailInvalid() *Error {
	return newError(http.StatusBadRequest, "email_invalid", i18n.MsgEmailInvalid)
}

// PasswordTooWeak 表示密码不满足 6.1 节规则。
func PasswordTooWeak() *Error {
	return newError(http.StatusBadRequest, "password_too_weak", i18n.MsgPasswordTooWeak)
}

// InvalidCredentials 是登录失败的唯一返回，不区分邮箱不存在与密码错误。
func InvalidCredentials() *Error {
	return newError(http.StatusUnauthorized, "invalid_credentials", i18n.MsgLoginFailed)
}

// EmailUnverified 表示邮箱尚未验证，前端据此展示「重新发送验证邮件」。
func EmailUnverified() *Error {
	return newError(http.StatusForbidden, "email_unverified", i18n.MsgEmailUnverified)
}

// AccountLocked 表示登录失败次数过多，账号被临时锁定。
func AccountLocked(retryAfter time.Duration) *Error {
	minutes := int((retryAfter + time.Minute - 1) / time.Minute)
	if minutes < 1 {
		minutes = 1
	}

	err := newError(http.StatusLocked, "account_locked", i18n.MsgRateLimited)
	err.Args = map[string]string{"n": strconv.Itoa(minutes), "unit": "分钟"}
	err.Details = map[string]any{"retry_after_seconds": int(retryAfter.Seconds())}
	return err
}

// AccountDisabled 表示账号被管理员停用。
func AccountDisabled() *Error {
	return newError(http.StatusForbidden, "account_disabled", i18n.MsgAccountDisabled)
}

// CodeInvalid 表示验证码错误，remaining 为剩余尝试次数。
func CodeInvalid(remaining int) *Error {
	err := newError(http.StatusBadRequest, "code_invalid", i18n.MsgCodeInvalid)
	err.Args = map[string]string{"n": strconv.Itoa(remaining)}
	err.Details = map[string]any{"attempts_remaining": remaining}
	return err
}

// CodeExpired 表示验证码已过期或尝试次数耗尽，两种情况前端处理一致：引导重新发送。
func CodeExpired() *Error {
	return newError(http.StatusGone, "code_expired", i18n.MsgCodeExpired)
}

// ResetLinkInvalid 表示重设密码链接过期或已被使用。
func ResetLinkInvalid() *Error {
	return newError(http.StatusGone, "reset_link_invalid", i18n.MsgResetLinkInvalid)
}

// CurrentPasswordInvalid 表示修改密码时当前密码不正确。
func CurrentPasswordInvalid() *Error {
	return newError(http.StatusBadRequest, "current_password_invalid", i18n.MsgCurrentPasswordBad)
}

// —— 身份收口到 Sub2API ——

// AuthMigrated 表示该端点承载的能力已迁到 Lumio 账号中心。
//
// 用 410 而不是删路由：存量客户端打过来时要能拿到「为什么没了、该去哪」，
// 404 会被误读成拼错路径，静默 200 更会让前端以为登录成功了。
func AuthMigrated(portalURL string) *Error {
	err := newError(http.StatusGone, "auth_migrated", i18n.MsgAuthMigrated)
	err.Args = map[string]string{"portal": portalURL}
	err.Details = map[string]any{
		"reason":     "identity_moved_to_lumio",
		"portal_url": portalURL,
	}
	return err
}

// IdentityUnavailable 表示无法向 Sub2API 求证调用者身份。
//
// 校验不了就明确失败，绝不放行；同时必须与 401 区分开，
// 否则上游抖一下就会把所有在线用户踢回登录页。
func IdentityUnavailable() *Error {
	return newError(http.StatusServiceUnavailable, "identity_unavailable", i18n.MsgIdentityUnavailable)
}

// IdempotencyKeyRequired 表示余额支付缺少 Idempotency-Key。
func IdempotencyKeyRequired() *Error {
	return newError(http.StatusBadRequest, "idempotency_key_required", i18n.MsgInvalidParams)
}

// InsufficientBalance 表示账户余额不够支付当前套餐。
//
// 给前端一条稳定的 code 和充值页地址；CC 响应没有 reason 字段。
func InsufficientBalance(purchaseURL string) *Error {
	err := newError(http.StatusForbidden, "insufficient_balance", i18n.MsgInsufficientBalance)
	err.Details = map[string]any{"purchase_url": purchaseURL}
	return err
}

// DebitUnavailable 表示余额扣费上游不可用。订单保持 pending，不得入账。
func DebitUnavailable() *Error {
	return newError(http.StatusServiceUnavailable, "debit_unavailable", i18n.MsgDebitUnavailable)
}

// DebitBusy 表示上游正在处理同一笔扣款。
func DebitBusy(retryAfter time.Duration) *Error {
	err := newError(http.StatusTooManyRequests, "debit_busy", i18n.MsgDebitBusy)
	if retryAfter > 0 {
		err.Details = map[string]any{"retry_after_seconds": int(retryAfter.Seconds())}
	}
	return err
}

// DebitMisconfigured 表示消费方密钥或 purpose 未被 LumioAPI 接受。
func DebitMisconfigured() *Error {
	return newError(http.StatusFailedDependency, "debit_misconfigured", i18n.MsgDebitMisconfigured)
}

// DebitIdempotencyConflict 表示同一订单号的请求体与首次不一致。
func DebitIdempotencyConflict() *Error {
	return newError(http.StatusConflict, "debit_idempotency_conflict", i18n.MsgDebitConflict)
}

// —— 订阅与邀请 ——

// TrialAlreadyUsed 表示该账号已享用过试用，或命中防滥用指纹。
func TrialAlreadyUsed() *Error {
	return newError(http.StatusConflict, "trial_already_used", i18n.MsgTrialAlreadyUsed)
}

// InviteCodeInvalid 表示邀请码不存在或已停用。
func InviteCodeInvalid() *Error {
	return newError(http.StatusNotFound, "invite_code_invalid", i18n.MsgInviteCodeInvalid)
}

// —— OAuth ——

// OAuthInvalidRequest 表示授权请求参数不合法。
func OAuthInvalidRequest(detail string) *Error {
	err := newError(http.StatusBadRequest, "invalid_request", i18n.MsgOAuthInvalidRequest)
	if detail != "" {
		err.Details = map[string]any{"reason": detail}
	}
	return err
}

// OAuthInvalidGrant 表示授权码或 refresh token 无效。
func OAuthInvalidGrant() *Error {
	return newError(http.StatusBadRequest, "invalid_grant", i18n.MsgOAuthInvalidGrant)
}

// —— 管理端 ——

// AdminMFARequired 表示需要补充两步验证。
func AdminMFARequired() *Error {
	return newError(http.StatusUnauthorized, "mfa_required", i18n.MsgAdminMFARequired)
}

// AdminMFAInvalid 表示两步验证码不正确。
func AdminMFAInvalid() *Error {
	return newError(http.StatusUnauthorized, "mfa_invalid", i18n.MsgAdminMFAInvalid)
}

// OrderNotRefundable 表示订单当前状态不允许退款。
func OrderNotRefundable() *Error {
	return newError(http.StatusConflict, "order_not_refundable", i18n.MsgOrderNotRefundable)
}
