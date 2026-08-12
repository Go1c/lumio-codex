package api

import (
	"io"
	"net/http"

	"github.com/go-chi/chi/v5"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/apperr"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/httpx"
)

func (s *Server) handlePlan(w http.ResponseWriter, r *http.Request) {
	plan, err := s.svc.Plan(r.Context())
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, plan)
}

type checkoutRequest struct {
	Channel        string `json:"channel"`
	IdempotencyKey string `json:"idempotency_key,omitempty"`
}

func (s *Server) handleCheckout(w http.ResponseWriter, r *http.Request) {
	var req checkoutRequest
	if err := httpx.DecodeJSON(w, r, &req); err != nil {
		httpx.Fail(w, r, err)
		return
	}

	result, err := s.svc.Checkout(r.Context(), principalOf(r).User.ID, req.Channel, req.IdempotencyKey)
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, result)
}

func (s *Server) handleListMyOrders(w http.ResponseWriter, r *http.Request) {
	orders, err := s.svc.ListMyOrders(r.Context(), principalOf(r).User.ID)
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, map[string]any{"items": orders})
}

func (s *Server) handleGetMyOrder(w http.ResponseWriter, r *http.Request) {
	order, err := s.svc.GetMyOrder(r.Context(), principalOf(r).User.ID, chi.URLParam(r, "orderNo"))
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, order)
}

// handleWebhook 接收支付渠道回调。签名放在 X-CCHaven-Signature 头，报文原样传给适配器验签。
func (s *Server) handleWebhook(w http.ResponseWriter, r *http.Request) {
	payload, err := io.ReadAll(http.MaxBytesReader(w, r.Body, 1<<20))
	if err != nil {
		httpx.Fail(w, r, apperr.InvalidParams().WithCause(err))
		return
	}

	if err := s.svc.HandleWebhook(r.Context(), chi.URLParam(r, "provider"),
		payload, r.Header.Get("X-CCHaven-Signature")); err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, map[string]string{"status": "ok"})
}
