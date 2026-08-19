package api

import (
	"net/http"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/apperr"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/httpx"
)

// handleAuthMigrated 应答所有已下线的自有终端用户认证端点。
//
// 注册、登录、验证码、找回密码、改邮箱、改口令全部搬到了 Lumio 账号中心
// （Sub2API）。路由保留、行为改为 410：存量客户端要能区分「这个能力永久没了、
// 该去哪」与「路径写错了」。响应体里的 portal_url 让前端可以直接引导跳转。
func (s *Server) handleAuthMigrated(w http.ResponseWriter, r *http.Request) {
	httpx.Fail(w, r, apperr.AuthMigrated(s.cfg.PortalLoginURL()))
}

// handleCheckout 把下单引导到 Sub2API 的统一充值页。
//
// CC 不再自建收银台：钱包与充值都在账号中心，CC 与 Codex 共用同一个入口。
// 同时给两种客户端留路——浏览器直接跟随 303 的 Location，XHR 读 data.purchase_url。
//
// 因此这个端点不再需要本地会话：目标是一个固定的公开地址，是否登录由充值页自己判定。
//
// checkout 仍只负责跳到 Sub2API 充值页。新的包月开通走 /billing/pay-with-balance，
// 订单会重新写入 orders；本端点不再建单。
func (s *Server) handleCheckout(w http.ResponseWriter, r *http.Request) {
	purchaseURL := s.cfg.PurchaseURL()

	w.Header().Set("Location", purchaseURL)
	httpx.JSON(w, http.StatusSeeOther, map[string]any{
		"purchase_url": purchaseURL,
		"reason":       "billing_moved_to_lumio",
	})
}
