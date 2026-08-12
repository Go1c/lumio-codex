// Package httpx 提供统一的请求解析与响应渲染。
//
// 成功：{"data": ...}；失败：{"error":{"code","message","details"}} + 语义化 HTTP 状态码。
// message 直接取自 i18n 字典，前端可原样展示；code 稳定不变，供前端自行本地化。
package httpx

import (
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net"
	"net/http"
	"strings"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/apperr"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/i18n"
)

const maxBodyBytes = 1 << 20 // 1 MiB，控制面没有大 body 场景

type envelope struct {
	Data any `json:"data,omitempty"`
}

type errorBody struct {
	Code    string         `json:"code"`
	Message string         `json:"message"`
	Details map[string]any `json:"details,omitempty"`
}

type errorEnvelope struct {
	Error errorBody `json:"error"`
}

// JSON 渲染成功响应。
func JSON(w http.ResponseWriter, status int, data any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(status)
	if data == nil {
		return
	}
	if err := json.NewEncoder(w).Encode(envelope{Data: data}); err != nil {
		slog.Error("写出响应失败", "error", err)
	}
}

// NoContent 渲染 204。
func NoContent(w http.ResponseWriter) { w.WriteHeader(http.StatusNoContent) }

// Fail 渲染错误响应。非 *apperr.Error 一律折叠为 500 并记日志，防止内部细节外泄。
func Fail(w http.ResponseWriter, r *http.Request, err error) {
	appErr := apperr.From(err)

	if appErr.Status >= http.StatusInternalServerError {
		slog.Error("请求处理失败",
			"method", r.Method, "path", r.URL.Path,
			"code", appErr.Code, "error", appErr.Error())
	}

	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(appErr.Status)

	body := errorEnvelope{Error: errorBody{
		Code:    appErr.Code,
		Message: i18n.T(LangOf(r), appErr.Message, appErr.Args),
		Details: appErr.Details,
	}}
	if encErr := json.NewEncoder(w).Encode(body); encErr != nil {
		slog.Error("写出错误响应失败", "error", encErr)
	}
}

// DecodeJSON 解析请求体，拒绝未知字段与超大 body。
func DecodeJSON(w http.ResponseWriter, r *http.Request, dst any) error {
	r.Body = http.MaxBytesReader(w, r.Body, maxBodyBytes)

	dec := json.NewDecoder(r.Body)
	dec.DisallowUnknownFields()

	if err := dec.Decode(dst); err != nil {
		return apperr.InvalidParams().WithCause(err)
	}
	// 拒绝一个请求体里塞多个 JSON 对象。
	if err := dec.Decode(&struct{}{}); !errors.Is(err, io.EOF) {
		return apperr.InvalidParams()
	}
	return nil
}

// LangOf 从 Accept-Language 推断界面语言，默认简体中文。
func LangOf(r *http.Request) i18n.Lang {
	if strings.Contains(strings.ToLower(r.Header.Get("Accept-Language")), "zh-hk") {
		return i18n.ZhHK
	}
	return i18n.ZhCN
}

// ClientIP 提取客户端 IP，优先使用反向代理写入的 X-Forwarded-For 首段。
func ClientIP(r *http.Request) string {
	if xff := r.Header.Get("X-Forwarded-For"); xff != "" {
		if first, _, found := strings.Cut(xff, ","); found {
			return strings.TrimSpace(first)
		}
		return strings.TrimSpace(xff)
	}
	if realIP := r.Header.Get("X-Real-IP"); realIP != "" {
		return strings.TrimSpace(realIP)
	}
	host, _, err := net.SplitHostPort(r.RemoteAddr)
	if err != nil {
		return r.RemoteAddr
	}
	return host
}

// UserAgent 返回截断后的 User-Agent，避免异常长的头污染数据库。
func UserAgent(r *http.Request) string {
	ua := r.UserAgent()
	if len(ua) > 512 {
		return ua[:512]
	}
	return ua
}
