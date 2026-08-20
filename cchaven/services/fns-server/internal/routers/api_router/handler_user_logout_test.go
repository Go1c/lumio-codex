package api_router

import (
	"context"
	"net/http"
	"testing"

	"github.com/haierkeys/fast-note-sync-service/internal/app"
	"github.com/haierkeys/fast-note-sync-service/internal/service"
	pkgapp "github.com/haierkeys/fast-note-sync-service/pkg/app"
	"github.com/haierkeys/fast-note-sync-service/pkg/code"
	"github.com/stretchr/testify/assert"
)

type logoutTokenService struct {
	service.TokenService
	revokeErr error
	uid       int64
	tokenID   int64
	calls     int
}

func (s *logoutTokenService) Revoke(_ context.Context, uid, tokenID int64) error {
	s.uid = uid
	s.tokenID = tokenID
	s.calls++
	return s.revokeErr
}

func newLogoutHandler(tokenService service.TokenService) *UserHandler {
	testApp := app.NewTestApp(&app.Services{TokenService: tokenService})
	return NewUserHandler(testApp)
}

func TestUserHandlerLogoutPropagatesRevocationFailure(t *testing.T) {
	tokenService := &logoutTokenService{
		revokeErr: code.ErrorDBQuery.WithDetails("forced revocation failure"),
	}
	handler := newLogoutHandler(tokenService)
	c, response := newUserTestContext(http.MethodPost, "/api/auth/logout", "", 0)
	c.Set("user_token", &pkgapp.UserEntity{UID: 41, TokenID: 73})

	handler.Logout(c)

	assertResponseCode(t, response, code.ErrorDBQuery.Code())
	assert.Equal(t, 1, tokenService.calls)
	assert.Equal(t, int64(41), tokenService.uid)
	assert.Equal(t, int64(73), tokenService.tokenID)
}

func TestUserHandlerLogoutReturnsSuccessAfterRevocation(t *testing.T) {
	tokenService := &logoutTokenService{}
	handler := newLogoutHandler(tokenService)
	c, response := newUserTestContext(http.MethodPost, "/api/auth/logout", "", 0)
	c.Set("user_token", &pkgapp.UserEntity{UID: 41, TokenID: 73})

	handler.Logout(c)

	assertResponseCode(t, response, code.Success.Code())
	assert.Equal(t, 1, tokenService.calls)
}
