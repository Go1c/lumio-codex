package api

import (
	"encoding/csv"
	"fmt"
	"net/http"
	"strconv"
	"time"

	"github.com/go-chi/chi/v5"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/apperr"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/httpx"
)

type adminLoginRequest struct {
	Email    string `json:"email"`
	Password string `json:"password"`
}

func (s *Server) handleAdminLogin(w http.ResponseWriter, r *http.Request) {
	var req adminLoginRequest
	if err := httpx.DecodeJSON(w, r, &req); err != nil {
		httpx.Fail(w, r, err)
		return
	}

	result, err := s.svc.AdminLogin(r.Context(), req.Email, req.Password,
		httpx.ClientIP(r), httpx.UserAgent(r))
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}

	// 会话令牌只进 HttpOnly cookie，不回传给 JS。
	s.writeCookie(w, s.cfg.CookieName.Admin, result.Token, s.cfg.AdminSessionTTL)
	httpx.JSON(w, http.StatusOK, result)
}

func (s *Server) handleAdminTOTP(w http.ResponseWriter, r *http.Request) {
	var req codeRequest
	if err := httpx.DecodeJSON(w, r, &req); err != nil {
		httpx.Fail(w, r, err)
		return
	}

	token, _ := s.adminTokenFrom(r)
	if err := s.svc.AdminVerifyTOTP(r.Context(), token, req.Code); err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, map[string]any{"mfa_passed": true})
}

func (s *Server) handleAdminLogout(w http.ResponseWriter, r *http.Request) {
	if err := s.svc.AdminLogout(r.Context(), adminOf(r).SessionID); err != nil {
		httpx.Fail(w, r, err)
		return
	}
	s.writeCookie(w, s.cfg.CookieName.Admin, "", -time.Hour)
	httpx.NoContent(w)
}

func (s *Server) handleAdminMe(w http.ResponseWriter, r *http.Request) {
	admin := adminOf(r).Admin
	httpx.JSON(w, http.StatusOK, map[string]any{
		"id":           admin.ID,
		"email":        admin.Email,
		"display_name": admin.DisplayName,
		"role":         admin.Role,
		"totp_enabled": admin.TOTPEnabled(),
	})
}

func (s *Server) handleAdminTOTPSetup(w http.ResponseWriter, r *http.Request) {
	enrollment, err := s.svc.AdminSetupTOTP(r.Context(), adminOf(r).Admin.ID)
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, enrollment)
}

func (s *Server) handleAdminTOTPEnable(w http.ResponseWriter, r *http.Request) {
	var req codeRequest
	if err := httpx.DecodeJSON(w, r, &req); err != nil {
		httpx.Fail(w, r, err)
		return
	}
	if err := s.svc.AdminEnableTOTP(r.Context(), adminOf(r).Admin.ID, req.Code); err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, map[string]any{"totp_enabled": true})
}

// —— 指标 ——

func (s *Server) handleMetricsOverview(w http.ResponseWriter, r *http.Request) {
	overview, err := s.svc.MetricsOverview(r.Context())
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, overview)
}

func (s *Server) handleMetricsDAU(w http.ResponseWriter, r *http.Request) {
	series, err := s.svc.DailyActive(r.Context(), intParam(r, "days", 7, 1, 90))
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, map[string]any{"items": series})
}

func (s *Server) handleMetricsDistributions(w http.ResponseWriter, r *http.Request) {
	distributions, err := s.svc.Distributions(r.Context(), intParam(r, "days", 30, 1, 365))
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, distributions)
}

// —— 用户 ——

func (s *Server) handleAdminListUsers(w http.ResponseWriter, r *http.Request) {
	status := r.URL.Query().Get("status")
	if status == "all" {
		status = ""
	}

	page := intParam(r, "page", 1, 1, 10000)
	pageSize := intParam(r, "page_size", 20, 1, 200)

	users, total, err := s.svc.ListUsers(r.Context(), r.URL.Query().Get("query"), status, page, pageSize)
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, map[string]any{
		"items": users, "total": total, "page": page, "page_size": pageSize,
	})
}

// handleAdminGetUser 返回用户详情。邮箱在此为明文，故受二次权限保护并逐次留痕。
func (s *Server) handleAdminGetUser(w http.ResponseWriter, r *http.Request) {
	userID, err := strconv.ParseInt(chi.URLParam(r, "id"), 10, 64)
	if err != nil {
		httpx.Fail(w, r, apperr.InvalidParams())
		return
	}

	detail, err := s.svc.UserDetail(r.Context(), adminOf(r), userID,
		httpx.ClientIP(r), httpx.UserAgent(r))
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, detail)
}

type disableUserRequest struct {
	Reason string `json:"reason,omitempty"`
}

func (s *Server) handleAdminDisableUser(w http.ResponseWriter, r *http.Request) {
	s.setUserDisabled(w, r, true)
}

func (s *Server) handleAdminEnableUser(w http.ResponseWriter, r *http.Request) {
	s.setUserDisabled(w, r, false)
}

func (s *Server) setUserDisabled(w http.ResponseWriter, r *http.Request, disabled bool) {
	userID, err := strconv.ParseInt(chi.URLParam(r, "id"), 10, 64)
	if err != nil {
		httpx.Fail(w, r, apperr.InvalidParams())
		return
	}

	req := disableUserRequest{}
	if r.ContentLength > 0 {
		if err := httpx.DecodeJSON(w, r, &req); err != nil {
			httpx.Fail(w, r, err)
			return
		}
	}

	if err := s.svc.SetUserDisabled(r.Context(), adminOf(r), userID, disabled, req.Reason,
		httpx.ClientIP(r), httpx.UserAgent(r)); err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, map[string]any{"disabled": disabled})
}

// —— 订单 ——

func (s *Server) handleAdminListOrders(w http.ResponseWriter, r *http.Request) {
	status := r.URL.Query().Get("status")
	if status == "all" {
		status = ""
	}

	page := intParam(r, "page", 1, 1, 10000)
	pageSize := intParam(r, "page_size", 20, 1, 200)

	orders, total, err := s.svc.ListOrders(r.Context(), status, page, pageSize)
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}

	todayCount, todayAmount, err := s.svc.TodayOrderTotals(r.Context())
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}

	httpx.JSON(w, http.StatusOK, map[string]any{
		"items": orders, "total": total, "page": page, "page_size": pageSize,
		"today": map[string]any{"count": todayCount, "amount_cents": todayAmount},
	})
}

// handleAdminExportOrders 按当前筛选导出 CSV。批量数据外带，权限与写操作同级。
func (s *Server) handleAdminExportOrders(w http.ResponseWriter, r *http.Request) {
	status := r.URL.Query().Get("status")
	if status == "all" {
		status = ""
	}

	orders, err := s.svc.ExportOrders(r.Context(), adminOf(r), status,
		httpx.ClientIP(r), httpx.UserAgent(r))
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}

	w.Header().Set("Content-Type", "text/csv; charset=utf-8")
	w.Header().Set("Content-Disposition", `attachment; filename="orders.csv"`)

	writer := csv.NewWriter(w)
	// BOM 让 Excel 正确识别 UTF-8 中文。
	_, _ = w.Write([]byte("\xEF\xBB\xBF"))
	_ = writer.Write([]string{"订单号", "用户邮箱", "金额", "币种", "支付渠道", "状态", "支付时间"})

	for _, o := range orders {
		paidAt := ""
		if o.PaidAt != nil {
			paidAt = o.PaidAt.Format("2006-01-02 15:04")
		}
		_ = writer.Write([]string{
			o.OrderNo, o.EmailMasked,
			fmt.Sprintf("%.2f", float64(o.AmountCents)/100),
			o.Currency, o.Channel, o.Status, paidAt,
		})
	}
	writer.Flush()
}

type refundRequest struct {
	Reason string `json:"reason,omitempty"`
}

func (s *Server) handleAdminRefund(w http.ResponseWriter, r *http.Request) {
	req := refundRequest{}
	if r.ContentLength > 0 {
		if err := httpx.DecodeJSON(w, r, &req); err != nil {
			httpx.Fail(w, r, err)
			return
		}
	}

	status, err := s.svc.RefundOrder(r.Context(), adminOf(r), chi.URLParam(r, "orderNo"),
		req.Reason, httpx.ClientIP(r), httpx.UserAgent(r))
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, map[string]any{"status": status})
}

// —— 运营配置与审计 ——

func (s *Server) handleAdminGetConfigs(w http.ResponseWriter, r *http.Request) {
	cfg, err := s.svc.OpsConfig(r.Context())
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, cfg)
}

func (s *Server) handleAdminPutConfigs(w http.ResponseWriter, r *http.Request) {
	var values map[string]any
	if err := httpx.DecodeJSON(w, r, &values); err != nil {
		httpx.Fail(w, r, err)
		return
	}
	if len(values) == 0 {
		httpx.Fail(w, r, apperr.InvalidParams())
		return
	}

	updated, err := s.svc.UpdateOpsConfig(r.Context(), adminOf(r), values,
		httpx.ClientIP(r), httpx.UserAgent(r))
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, updated)
}

func (s *Server) handleAdminAuditLogs(w http.ResponseWriter, r *http.Request) {
	page := intParam(r, "page", 1, 1, 10000)
	pageSize := intParam(r, "page_size", 50, 1, 200)

	logs, total, err := s.svc.AuditLogs(r.Context(),
		r.URL.Query().Get("actor"), r.URL.Query().Get("action"), page, pageSize)
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, map[string]any{
		"items": logs, "total": total, "page": page, "page_size": pageSize,
	})
}

// intParam 读取整型查询参数并夹到 [min, max] 区间。
func intParam(r *http.Request, name string, fallback, minValue, maxValue int) int {
	raw := r.URL.Query().Get(name)
	if raw == "" {
		return fallback
	}

	value, err := strconv.Atoi(raw)
	if err != nil {
		return fallback
	}
	return max(minValue, min(maxValue, value))
}
