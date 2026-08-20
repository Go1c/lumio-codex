package middleware

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/gin-gonic/gin"
	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/haierkeys/fast-note-sync-service/pkg/app"
	"github.com/haierkeys/fast-note-sync-service/pkg/code"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

type fakeMiddlewareTokenService struct {
	activeToken *domain.AuthToken
	activeErr   error
	lookupUID   int64
	lookupID    int64
	lookupCalls int
}

func (s *fakeMiddlewareTokenService) Create(ctx context.Context, uid int64, params *dto.TokenIssueRequest) (*dto.TokenCreateResponse, error) {
	return nil, errors.New("not implemented")
}

func (s *fakeMiddlewareTokenService) CreateForLogin(ctx context.Context, uid int64, clientType, ip, userAgent string) (*domain.AuthToken, string, error) {
	return nil, "", errors.New("not implemented")
}

func (s *fakeMiddlewareTokenService) ListByUser(ctx context.Context, uid int64) ([]*dto.TokenResponse, error) {
	return nil, errors.New("not implemented")
}

func (s *fakeMiddlewareTokenService) Update(ctx context.Context, uid int64, tokenID int64, params *dto.TokenUpdateRequest) error {
	return errors.New("not implemented")
}

func (s *fakeMiddlewareTokenService) Revoke(ctx context.Context, uid int64, tokenID int64) error {
	return errors.New("not implemented")
}

func (s *fakeMiddlewareTokenService) Rotate(ctx context.Context, uid int64, tokenID int64) (*dto.TokenCreateResponse, error) {
	return nil, errors.New("not implemented")
}

func (s *fakeMiddlewareTokenService) RotateForLogin(ctx context.Context, uid int64, tokenID int64, ip, userAgent string) (*domain.AuthToken, string, error) {
	return nil, "", errors.New("not implemented")
}

func (s *fakeMiddlewareTokenService) GetActiveToken(ctx context.Context, uid int64, tokenID int64) (*domain.AuthToken, error) {
	s.lookupUID = uid
	s.lookupID = tokenID
	s.lookupCalls++
	return s.activeToken, s.activeErr
}

func (s *fakeMiddlewareTokenService) RecordAccessLog(ctx context.Context, log *domain.AuthTokenLog) error {
	return nil
}

func (s *fakeMiddlewareTokenService) ListLogs(ctx context.Context, uid, tokenID int64, page, pageSize int) ([]*dto.TokenLogResponse, int64, error) {
	return nil, 0, errors.New("not implemented")
}

func (s *fakeMiddlewareTokenService) UpdateLastUsedAt(ctx context.Context, tokenID int64) error {
	return errors.New("not implemented")
}

func (s *fakeMiddlewareTokenService) SetSyncHandler(handler func(uid int64, tokenID int64, scope string, kick bool)) {
}

func (s *fakeMiddlewareTokenService) GetRecentClients(ctx context.Context, uid int64, duration time.Duration) (map[int64][]string, error) {
	return nil, nil
}

func newMiddlewareJWT(t *testing.T, secretKey, nonce string) string {
	t.Helper()
	tokenManager := app.NewTokenManager(app.TokenConfig{
		SecretKey: secretKey,
		Expiry:    time.Hour,
	})
	token, err := tokenManager.Generate(1, "", "", 2, nonce)
	require.NoError(t, err)
	return token
}

func runUserAuthMiddleware(t *testing.T, tokenService *fakeMiddlewareTokenService, token string) app.Res {
	return runUserAuthMiddlewareWithRequest(t, tokenService, token, http.MethodGet, "/api/note/list?path=test.md", func(req *http.Request) {
		req.Header.Set("x-client", "ObsidianPlugin")
		req.Header.Set("User-Agent", "Obsidian")
	})
}

func runUserAuthMiddlewareWithRequest(t *testing.T, tokenService *fakeMiddlewareTokenService, token string, method string, target string, configure func(*http.Request)) app.Res {
	t.Helper()
	gin.SetMode(gin.TestMode)

	router := gin.New()
	router.Use(UserAuthTokenWithConfig("test-secret", tokenService))
	router.GET("/api/note/list", func(c *gin.Context) {
		c.JSON(http.StatusOK, gin.H{"code": code.Success.Code(), "status": true})
	})
	router.GET("/api/file", func(c *gin.Context) {
		c.JSON(http.StatusOK, gin.H{"code": code.Success.Code(), "status": true})
	})
	router.POST("/api/file", func(c *gin.Context) {
		c.JSON(http.StatusOK, gin.H{"code": code.Success.Code(), "status": true})
	})

	req := httptest.NewRequest(method, target, nil)
	req.Header.Set("Authorization", "Bearer "+token)
	if configure != nil {
		configure(req)
	}
	recorder := httptest.NewRecorder()

	router.ServeHTTP(recorder, req)

	require.Equal(t, http.StatusOK, recorder.Code)
	var res app.Res
	require.NoError(t, json.Unmarshal(recorder.Body.Bytes(), &res))
	return res
}

func TestUserAuthTokenWithConfig_AllowsValidManualTokenScope(t *testing.T) {
	token := newMiddlewareJWT(t, "test-secret", "nonce-ok")
	res := runUserAuthMiddleware(t, &fakeMiddlewareTokenService{activeToken: &domain.AuthToken{
		ID:          2,
		UID:         1,
		TokenString: "nonce-ok",
		Status:      1,
		Scope:       "p:rest,ws c:ObsidianPlugin f:note_rw,file_rw,config_rw",
		IssueType:   2,
		ExpiredAt:   time.Now().Add(time.Hour),
	}}, token)

	assert.Equal(t, code.Success.Code(), res.Code)
}

func TestUserAuthTokenWithConfig_PropagatesInvalidStatefulToken(t *testing.T) {
	token := newMiddlewareJWT(t, "test-secret", "stale-nonce")
	res := runUserAuthMiddleware(t, &fakeMiddlewareTokenService{
		activeErr: code.ErrorInvalidUserAuthToken.WithDetails("Token has been revoked or no longer exists"),
	}, token)

	assert.Equal(t, code.ErrorInvalidUserAuthToken.Code(), res.Code)
	assert.Contains(t, res.Details, "revoked")
}

func TestUserAuthTokenWithConfig_PropagatesExpiredToken(t *testing.T) {
	token := newMiddlewareJWT(t, "test-secret", "expired-nonce")
	res := runUserAuthMiddleware(t, &fakeMiddlewareTokenService{
		activeErr: code.ErrorTokenExpired,
	}, token)

	assert.Equal(t, code.ErrorTokenExpired.Code(), res.Code)
}

func TestUserAuthTokenWithConfig_RejectsNonceMismatch(t *testing.T) {
	token := newMiddlewareJWT(t, "test-secret", "old-nonce")
	res := runUserAuthMiddleware(t, &fakeMiddlewareTokenService{activeToken: &domain.AuthToken{
		ID:          2,
		UID:         1,
		TokenString: "new-nonce",
		Status:      1,
		Scope:       "p:rest,ws c:ObsidianPlugin f:note_rw,file_rw,config_rw",
		IssueType:   2,
		ExpiredAt:   time.Now().Add(time.Hour),
	}}, token)

	assert.Equal(t, code.ErrorInvalidUserAuthToken.Code(), res.Code)
	assert.Contains(t, res.Details, "rotated")
}

func TestUserAuthTokenWithConfig_RejectsScopeRestrictedToken(t *testing.T) {
	token := newMiddlewareJWT(t, "test-secret", "nonce-ok")
	res := runUserAuthMiddleware(t, &fakeMiddlewareTokenService{activeToken: &domain.AuthToken{
		ID:          2,
		UID:         1,
		TokenString: "nonce-ok",
		Status:      1,
		Scope:       "p:ws c:ObsidianPlugin f:note_rw",
		IssueType:   2,
		ExpiredAt:   time.Now().Add(time.Hour),
	}}, token)

	assert.Equal(t, code.ErrorAuthTokenScopeRestricted.Code(), res.Code)
	assert.Contains(t, res.Details, "Permission denied")
}

func TestUserAuthTokenWithConfig_AllowsLoginTokenWithoutClientHeader(t *testing.T) {
	token := newMiddlewareJWT(t, "test-secret", "nonce-ok")
	res := runUserAuthMiddlewareWithRequest(t, &fakeMiddlewareTokenService{activeToken: &domain.AuthToken{
		ID:          2,
		UID:         1,
		TokenString: "nonce-ok",
		Status:      1,
		Scope:       "p:rest c:WebGui f:*",
		ClientType:  "WebGui",
		IssueType:   1,
		ExpiredAt:   time.Now().Add(time.Hour),
	}}, token, http.MethodGet, "/api/file?vault=main&path=image.png", func(req *http.Request) {
		req.Header.Set("User-Agent", "Mozilla/5.0")
	})

	assert.Equal(t, code.Success.Code(), res.Code)
}

func TestUserAuthTokenWithConfig_RejectsHeaderlessLoginTokenWrite(t *testing.T) {
	token := newMiddlewareJWT(t, "test-secret", "nonce-ok")
	res := runUserAuthMiddlewareWithRequest(t, &fakeMiddlewareTokenService{activeToken: &domain.AuthToken{
		ID:          2,
		UID:         1,
		TokenString: "nonce-ok",
		Status:      1,
		Scope:       "p:rest c:WebGui f:*",
		ClientType:  "WebGui",
		IssueType:   1,
		ExpiredAt:   time.Now().Add(time.Hour),
	}}, token, http.MethodPost, "/api/file?vault=main&path=image.png", func(req *http.Request) {
		req.Header.Set("User-Agent", "Mozilla/5.0")
	})

	assert.Equal(t, code.ErrorAuthTokenClientRestricted.Code(), res.Code)
}

func TestUserAuthTokenWithConfig_RejectsManualTokenWithoutClientHeader(t *testing.T) {
	token := newMiddlewareJWT(t, "test-secret", "nonce-ok")
	res := runUserAuthMiddlewareWithRequest(t, &fakeMiddlewareTokenService{activeToken: &domain.AuthToken{
		ID:          2,
		UID:         1,
		TokenString: "nonce-ok",
		Status:      1,
		Scope:       "p:rest c:WebGui f:file_r",
		ClientType:  "WebGui",
		IssueType:   2,
		ExpiredAt:   time.Now().Add(time.Hour),
	}}, token, http.MethodGet, "/api/file?vault=main&path=image.png", nil)

	assert.Equal(t, code.ErrorAuthTokenScopeRestricted.Code(), res.Code)
}

// TestUserAuthTokenWithConfig_InjectsTokenContextAttributes verifies that UserAuthTokenWithConfig
// correctly injects token_issue_type and token_client_type into gin.Context after successful authentication.
// These context values are consumed by middleware.RequireWebGUI for multi-factor verification.
// 验证 UserAuthTokenWithConfig 在认证成功后将 token_issue_type 和 token_client_type
// 正确注入到 gin.Context，这些值供 RequireWebGUI 进行联合校验防止请求头伪造
func TestUserAuthTokenWithConfig_InjectsTokenContextAttributes(t *testing.T) {
	gin.SetMode(gin.TestMode)

	tokenSvc := &fakeMiddlewareTokenService{activeToken: &domain.AuthToken{
		ID:          3,
		UID:         1,
		TokenString: "nonce-inject",
		Status:      1,
		Scope:       "p:rest c:WebGui f:*",
		ClientType:  "WebGui",
		IssueType:   1,
		ExpiredAt:   time.Now().Add(time.Hour),
	}}

	var capturedIssueType interface{}
	var capturedClientType interface{}

	router := gin.New()
	router.Use(UserAuthTokenWithConfig("test-secret", tokenSvc))
	router.GET("/api/note/list", func(c *gin.Context) {
		capturedIssueType, _ = c.Get("token_issue_type")
		capturedClientType, _ = c.Get("token_client_type")
		c.JSON(http.StatusOK, gin.H{"code": code.Success.Code()})
	})

	token, err := app.NewTokenManager(app.TokenConfig{
		SecretKey: "test-secret",
		Expiry:    time.Hour,
	}).Generate(1, "", "", 1, "nonce-inject")
	require.NoError(t, err)

	req := httptest.NewRequest(http.MethodGet, "/api/note/list?path=test.md", nil)
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("x-client", "WebGui")

	w := httptest.NewRecorder()
	router.ServeHTTP(w, req)

	require.Equal(t, http.StatusOK, w.Code)
	// Verify token_issue_type injected correctly // 验证 token_issue_type 注入正确
	assert.Equal(t, 1, capturedIssueType, "token_issue_type should be 1 (Login)")
	// Verify token_client_type injected correctly // 验证 token_client_type 注入正确
	assert.Equal(t, "WebGui", capturedClientType, "token_client_type should match dbToken.ClientType")
}

func TestAuthenticateBearerUserTokenForProtocolRejectsFallbackSources(t *testing.T) {
	token := newMiddlewareJWT(t, "test-secret", "nonce-v2")
	tokenService := &fakeMiddlewareTokenService{activeToken: &domain.AuthToken{
		ID:          2,
		UID:         1,
		TokenString: "nonce-v2",
		Status:      1,
		Scope:       "p:ws c:fns-agent f:*",
		ExpiredAt:   time.Now().Add(time.Hour),
	}}

	cases := map[string]func(*http.Request){
		"Token header":           func(req *http.Request) { req.Header.Set("Token", token) },
		"lowercase token header": func(req *http.Request) { req.Header.Set("token", token) },
		"query token":            func(req *http.Request) { req.URL.RawQuery = "token=" + token },
	}
	for name, configure := range cases {
		t.Run(name, func(t *testing.T) {
			c := newProtocolAuthContext(t, configure)
			_, appErr := AuthenticateBearerUserTokenForProtocol(c, "test-secret", tokenService, "ws", "fns-agent", "workspace_rw")
			require.NotNil(t, appErr)
			require.Equal(t, code.ErrorNotUserAuthToken.Code(), appErr.Code())
		})
	}
}

func TestAuthenticateBearerUserTokenForProtocolRequiresSpaceSeparator(t *testing.T) {
	token := newMiddlewareJWT(t, "test-secret", "nonce-v2")
	tests := []struct {
		name    string
		header  string
		allowed bool
	}{
		{name: "single space", header: "Bearer " + token, allowed: true},
		{name: "multiple spaces", header: "Bearer  " + token, allowed: true},
		{name: "tab separator", header: "Bearer\t" + token},
		{name: "non-breaking-space separator", header: "Bearer\u00a0" + token},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			tokenService := &fakeMiddlewareTokenService{activeToken: &domain.AuthToken{
				ID:          2,
				UID:         1,
				TokenString: "nonce-v2",
				Status:      1,
				Scope:       "p:ws c:fns-agent f:workspace_rw",
				ExpiredAt:   time.Now().Add(time.Hour),
			}}
			c := newProtocolAuthContext(t, func(req *http.Request) {
				req.Header.Set("Authorization", tt.header)
				req.Header.Set("X-Client", "fns-agent")
			})

			identity, appErr := AuthenticateBearerUserTokenForProtocol(c, "test-secret", tokenService, "ws", "fns-agent", "workspace_rw")
			if tt.allowed {
				require.Nil(t, appErr)
				require.NotNil(t, identity)
				require.Equal(t, 1, tokenService.lookupCalls)
				return
			}
			require.Nil(t, identity)
			require.NotNil(t, appErr)
			require.Equal(t, code.ErrorNotUserAuthToken.Code(), appErr.Code())
			require.Zero(t, tokenService.lookupCalls)
		})
	}
}

func TestAuthenticateBearerUserTokenForProtocolAcceptsValidWSScope(t *testing.T) {
	token := newMiddlewareJWT(t, "test-secret", "nonce-v2")
	tokenService := &fakeMiddlewareTokenService{activeToken: &domain.AuthToken{
		ID:          2,
		UID:         1,
		TokenString: "nonce-v2",
		Status:      1,
		Scope:       "p:ws c:fns-agent f:workspace_rw",
		BoundIP:     "192.0.2.1",
		UserAgent:   "FNS-Agent/*",
		ClientType:  "fns-agent",
		ExpiredAt:   time.Now().Add(time.Hour),
	}}
	c := newProtocolAuthContext(t, func(req *http.Request) {
		req.Header.Set("Authorization", "Bearer "+token)
		req.Header.Set("X-Client", "fns-agent")
		req.Header.Set("X-Client-Name", "desktop")
		req.Header.Set("X-Client-Version", "2.0")
		req.Header.Set("User-Agent", "FNS-Agent/2")
	})

	identity, appErr := AuthenticateBearerUserTokenForProtocol(c, "test-secret", tokenService, "ws", "fns-agent", "workspace_rw")
	require.Nil(t, appErr)
	require.NotNil(t, identity)
	require.Equal(t, int64(1), identity.User.UID)
	require.Equal(t, "p:ws c:fns-agent f:workspace_rw", identity.Scope)
	require.Equal(t, "fns-agent", identity.ClientType)
	require.Equal(t, "desktop", identity.ClientName)
	require.Equal(t, "2.0", identity.ClientVersion)
}

func TestAuthenticateBearerUserTokenForProtocolRejectsRESTOnlyScope(t *testing.T) {
	token := newMiddlewareJWT(t, "test-secret", "nonce-v2")
	tokenService := &fakeMiddlewareTokenService{activeToken: &domain.AuthToken{
		ID:          2,
		UID:         1,
		TokenString: "nonce-v2",
		Status:      1,
		Scope:       "p:rest c:fns-agent f:workspace_rw",
		ExpiredAt:   time.Now().Add(time.Hour),
	}}
	c := newProtocolAuthContext(t, func(req *http.Request) {
		req.Header.Set("Authorization", "Bearer "+token)
		req.Header.Set("X-Client", "fns-agent")
	})

	_, appErr := AuthenticateBearerUserTokenForProtocol(c, "test-secret", tokenService, "ws", "fns-agent", "workspace_rw")
	require.NotNil(t, appErr)
	require.Equal(t, code.ErrorAuthTokenScopeRestricted.Code(), appErr.Code())
}

func TestAuthenticateBearerUserTokenForProtocolRequiresExactWorkspacePermission(t *testing.T) {
	token := newMiddlewareJWT(t, "test-secret", "nonce-v2")
	tests := []struct {
		name          string
		scope         string
		requestClient string
		allowed       bool
	}{
		{name: "exact workspace permission", scope: "p:ws c:fns-agent f:workspace_rw", allowed: true},
		{name: "REST protocol", scope: "p:rest c:fns-agent f:workspace_rw"},
		{name: "wrong client", scope: "p:ws c:other-agent f:workspace_rw"},
		{name: "matching non-agent client", scope: "p:ws c:other-agent f:workspace_rw", requestClient: "other-agent"},
		{name: "missing function", scope: "p:ws c:fns-agent"},
		{name: "wrong function", scope: "p:ws c:fns-agent f:note_rw"},
		{name: "blank scope", scope: ""},
		{name: "wildcard function", scope: "p:ws c:fns-agent f:*"},
		{name: "additional function", scope: "p:ws c:fns-agent f:workspace_rw,note_rw"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			requestClient := tt.requestClient
			if requestClient == "" {
				requestClient = "fns-agent"
			}
			tokenService := &fakeMiddlewareTokenService{activeToken: &domain.AuthToken{
				ID:          2,
				UID:         1,
				TokenString: "nonce-v2",
				Status:      1,
				Scope:       tt.scope,
				ClientType:  "fns-agent",
				IssueType:   2,
				ExpiredAt:   time.Now().Add(time.Hour),
			}}
			c := newProtocolAuthContext(t, func(req *http.Request) {
				req.Header.Set("Authorization", "Bearer "+token)
				req.Header.Set("X-Client", requestClient)
			})

			identity, appErr := AuthenticateBearerUserTokenForProtocol(c, "test-secret", tokenService, "ws", "fns-agent", "workspace_rw")
			if tt.allowed {
				require.Nil(t, appErr)
				require.NotNil(t, identity)
				return
			}
			require.Nil(t, identity)
			require.NotNil(t, appErr)
			require.Equal(t, code.ErrorAuthTokenScopeRestricted.Code(), appErr.Code())
		})
	}
}

func TestAuthenticateBearerUserTokenForProtocolRetainsJWTAndStatefulTokenChecks(t *testing.T) {
	validToken := newMiddlewareJWT(t, "test-secret", "nonce-v2")
	activeToken := func() *domain.AuthToken {
		return &domain.AuthToken{
			ID:          2,
			UID:         1,
			TokenString: "nonce-v2",
			Status:      1,
			Scope:       "p:ws c:fns-agent f:workspace_rw",
			ClientType:  "fns-agent",
			IssueType:   2,
			ExpiredAt:   time.Now().Add(time.Hour),
		}
	}
	newContext := func(token string) *gin.Context {
		return newProtocolAuthContext(t, func(req *http.Request) {
			req.Header.Set("Authorization", "Bearer "+token)
			req.Header.Set("X-Client", "fns-agent")
		})
	}

	t.Run("active row uses JWT UID and token ID", func(t *testing.T) {
		tokenService := &fakeMiddlewareTokenService{activeToken: activeToken()}
		identity, appErr := AuthenticateBearerUserTokenForProtocol(newContext(validToken), "test-secret", tokenService, "ws", "fns-agent", "workspace_rw")
		require.Nil(t, appErr)
		require.NotNil(t, identity)
		require.Equal(t, int64(1), tokenService.lookupUID)
		require.Equal(t, int64(2), tokenService.lookupID)
		require.Equal(t, 1, tokenService.lookupCalls)
	})

	t.Run("invalid JWT signature", func(t *testing.T) {
		tokenService := &fakeMiddlewareTokenService{activeToken: activeToken()}
		identity, appErr := AuthenticateBearerUserTokenForProtocol(newContext(newMiddlewareJWT(t, "wrong-secret", "nonce-v2")), "test-secret", tokenService, "ws", "fns-agent", "workspace_rw")
		require.Nil(t, identity)
		require.NotNil(t, appErr)
		require.Equal(t, code.ErrorInvalidUserAuthToken.Code(), appErr.Code())
		require.Zero(t, tokenService.lookupCalls)
	})

	t.Run("expired JWT", func(t *testing.T) {
		expiredToken, err := app.NewTokenManager(app.TokenConfig{
			SecretKey: "test-secret",
			Expiry:    -time.Hour,
		}).Generate(1, "", "", 2, "nonce-v2")
		require.NoError(t, err)
		tokenService := &fakeMiddlewareTokenService{activeToken: activeToken()}
		identity, appErr := AuthenticateBearerUserTokenForProtocol(newContext(expiredToken), "test-secret", tokenService, "ws", "fns-agent", "workspace_rw")
		require.Nil(t, identity)
		require.NotNil(t, appErr)
		// The legacy signing-key fallback may normalize expiry to invalid token;
		// both codes remain unauthorized and must stop before state lookup.
		require.Contains(t, []int{code.ErrorTokenExpired.Code(), code.ErrorInvalidUserAuthToken.Code()}, appErr.Code())
		require.Zero(t, tokenService.lookupCalls)
	})

	tests := []struct {
		name        string
		activeToken *domain.AuthToken
		activeErr   error
		want        *code.Code
	}{
		{
			name:      "revoked or missing database row",
			activeErr: code.ErrorInvalidUserAuthToken.WithDetails("Token has been revoked or no longer exists"),
			want:      code.ErrorInvalidUserAuthToken,
		},
		{
			name:      "expired database row",
			activeErr: code.ErrorTokenExpired,
			want:      code.ErrorTokenExpired,
		},
		{
			name: "nonce mismatch after rotation",
			activeToken: func() *domain.AuthToken {
				candidate := activeToken()
				candidate.TokenString = "rotated-nonce"
				return candidate
			}(),
			want: code.ErrorInvalidUserAuthToken,
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			tokenService := &fakeMiddlewareTokenService{activeToken: tt.activeToken, activeErr: tt.activeErr}
			identity, appErr := AuthenticateBearerUserTokenForProtocol(newContext(validToken), "test-secret", tokenService, "ws", "fns-agent", "workspace_rw")
			require.Nil(t, identity)
			require.NotNil(t, appErr)
			require.Equal(t, tt.want.Code(), appErr.Code())
			require.Equal(t, 1, tokenService.lookupCalls)
		})
	}
}

func TestAuthenticateBearerUserTokenForProtocolRejectsClientIPAndUserAgentMismatch(t *testing.T) {
	token := newMiddlewareJWT(t, "test-secret", "nonce-v2")
	cases := map[string]struct {
		boundIP   string
		userAgent string
		requestIP string
		requestUA string
		want      *code.Code
	}{
		"ip": {
			boundIP: "192.0.2.2", userAgent: "FNS-Agent/*", requestIP: "192.0.2.1", requestUA: "FNS-Agent/2",
			want: code.ErrorAuthTokenIPRestricted,
		},
		"user agent": {
			boundIP: "192.0.2.1", userAgent: "Other/*", requestIP: "192.0.2.1", requestUA: "FNS-Agent/2",
			want: code.ErrorAuthTokenUARestricted,
		},
	}
	for name, candidate := range cases {
		t.Run(name, func(t *testing.T) {
			tokenService := &fakeMiddlewareTokenService{activeToken: &domain.AuthToken{
				ID:          2,
				UID:         1,
				TokenString: "nonce-v2",
				Status:      1,
				Scope:       "p:ws c:fns-agent f:workspace_rw",
				BoundIP:     candidate.boundIP,
				UserAgent:   candidate.userAgent,
				ExpiredAt:   time.Now().Add(time.Hour),
			}}
			c := newProtocolAuthContext(t, func(req *http.Request) {
				req.RemoteAddr = candidate.requestIP + ":1234"
				req.Header.Set("Authorization", "Bearer "+token)
				req.Header.Set("X-Client", "fns-agent")
				req.Header.Set("User-Agent", candidate.requestUA)
			})

			_, appErr := AuthenticateBearerUserTokenForProtocol(c, "test-secret", tokenService, "ws", "fns-agent", "workspace_rw")
			require.NotNil(t, appErr)
			require.Equal(t, candidate.want.Code(), appErr.Code())
		})
	}
}

func TestAuthenticateUserTokenRetainsTokenHeaderAndQueryFallbackForREST(t *testing.T) {
	token := newMiddlewareJWT(t, "test-secret", "nonce-rest")
	cases := map[string]func(*http.Request){
		"Token header":           func(req *http.Request) { req.Header.Set("Token", token) },
		"lowercase token header": func(req *http.Request) { req.Header.Set("token", token) },
		"query token":            func(req *http.Request) { req.URL.RawQuery = "token=" + token },
	}
	for name, configure := range cases {
		t.Run(name, func(t *testing.T) {
			tokenService := &fakeMiddlewareTokenService{activeToken: &domain.AuthToken{
				ID:          2,
				UID:         1,
				TokenString: "nonce-rest",
				Status:      1,
				Scope:       "p:rest c:ObsidianPlugin f:*",
				ExpiredAt:   time.Now().Add(time.Hour),
			}}
			c := newProtocolAuthContext(t, func(req *http.Request) {
				configure(req)
				req.Header.Set("X-Client", "ObsidianPlugin")
				req.Header.Set("User-Agent", "Obsidian")
			})

			user, _, _, _, appErr := AuthenticateUserToken(c, "test-secret", tokenService)
			require.Nil(t, appErr)
			require.NotNil(t, user)
		})
	}
}

func newProtocolAuthContext(t *testing.T, configure func(*http.Request)) *gin.Context {
	t.Helper()
	req := httptest.NewRequest(http.MethodGet, "/api/user/workspace-sync/v2", nil)
	req.RemoteAddr = "192.0.2.1:1234"
	if configure != nil {
		configure(req)
	}
	gin.SetMode(gin.TestMode)
	c, _ := gin.CreateTestContext(httptest.NewRecorder())
	c.Request = req
	return c
}
