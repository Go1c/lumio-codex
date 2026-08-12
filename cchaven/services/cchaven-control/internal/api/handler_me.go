package api

import (
	"net/http"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/apperr"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/httpx"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/service"
)

func (s *Server) handleMe(w http.ResponseWriter, r *http.Request) {
	principal := principalOf(r)

	entitlement, err := s.svc.Entitlement(r.Context(), principal.User.ID)
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, map[string]any{
		"user":        service.ViewUser(principal.User),
		"entitlement": entitlement,
	})
}

type updateProfileRequest struct {
	DisplayName string `json:"display_name"`
}

func (s *Server) handleUpdateProfile(w http.ResponseWriter, r *http.Request) {
	var req updateProfileRequest
	if err := httpx.DecodeJSON(w, r, &req); err != nil {
		httpx.Fail(w, r, err)
		return
	}

	view, err := s.svc.UpdateProfile(r.Context(), principalOf(r).User.ID, req.DisplayName)
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, view)
}

func (s *Server) handleEntitlement(w http.ResponseWriter, r *http.Request) {
	entitlement, err := s.svc.Entitlement(r.Context(), principalOf(r).User.ID)
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, entitlement)
}

func (s *Server) handleListSessions(w http.ResponseWriter, r *http.Request) {
	principal := principalOf(r)

	sessions, err := s.svc.ListSessions(r.Context(), principal.User.ID, principal.SessionID)
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, map[string]any{"items": sessions})
}

func (s *Server) handleRevokeSession(w http.ResponseWriter, r *http.Request) {
	id, err := uuid.Parse(chi.URLParam(r, "id"))
	if err != nil {
		httpx.Fail(w, r, apperr.InvalidParams())
		return
	}
	if err := s.svc.RevokeSession(r.Context(), principalOf(r).User.ID, id); err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.NoContent(w)
}

func (s *Server) handleRevokeOtherSessions(w http.ResponseWriter, r *http.Request) {
	principal := principalOf(r)

	count, err := s.svc.RevokeOtherSessions(r.Context(), principal.User.ID, principal.SessionID)
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, map[string]any{"revoked": count})
}

func (s *Server) handleReferrals(w http.ResponseWriter, r *http.Request) {
	overview, err := s.svc.ReferralOverviewFor(r.Context(), principalOf(r).User.ID)
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, overview)
}

func (s *Server) handleRequestDeletion(w http.ResponseWriter, r *http.Request) {
	effective, err := s.svc.RequestAccountDeletion(r.Context(), principalOf(r).User.ID)
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, map[string]any{"effective_at": effective})
}

func (s *Server) handleCancelDeletion(w http.ResponseWriter, r *http.Request) {
	if err := s.svc.CancelAccountDeletion(r.Context(), principalOf(r).User.ID); err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.NoContent(w)
}

type heartbeatRequest struct {
	DeviceID   string `json:"device_id"`
	AppVersion string `json:"app_version"`
	OSVersion  string `json:"os_version"`
	Arch       string `json:"arch"`
}

func (s *Server) handleHeartbeat(w http.ResponseWriter, r *http.Request) {
	var req heartbeatRequest
	if err := httpx.DecodeJSON(w, r, &req); err != nil {
		httpx.Fail(w, r, err)
		return
	}

	principal := principalOf(r)
	result, err := s.svc.Heartbeat(r.Context(), service.HeartbeatInput{
		UserID:     principal.User.ID,
		SessionID:  principal.SessionID,
		DeviceID:   req.DeviceID,
		AppVersion: req.AppVersion,
		OSVersion:  req.OSVersion,
		Arch:       req.Arch,
	})
	if err != nil {
		httpx.Fail(w, r, err)
		return
	}
	httpx.JSON(w, http.StatusOK, result)
}
