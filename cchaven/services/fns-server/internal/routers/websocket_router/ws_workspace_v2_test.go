package websocket_router

import (
	"bufio"
	"context"
	"errors"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/gin-gonic/gin"
	internalapp "github.com/haierkeys/fast-note-sync-service/internal/app"
	"github.com/haierkeys/fast-note-sync-service/internal/config"
	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/middleware"
	"github.com/haierkeys/fast-note-sync-service/internal/service"
	pkgapp "github.com/haierkeys/fast-note-sync-service/pkg/app"
	"github.com/haierkeys/fast-note-sync-service/pkg/code"
	"github.com/lxzan/gws"
	"github.com/stretchr/testify/require"
)

const workspaceV2AuthTestSecret = "workspace-v2-auth-test-secret"

func TestWorkspaceV2UpgradeRequiresBearerBeforeSwitchingProtocols(t *testing.T) {
	server, httpServer := newWorkspaceV2HTTPTestServer(t, func(c *gin.Context) (*middleware.AuthenticatedUserToken, *code.Code) {
		if c.GetHeader("Authorization") == "" {
			return nil, code.ErrorNotUserAuthToken
		}
		return workspaceV2TestIdentity(), nil
	})
	_ = server

	response, err := http.Get(httpServer.URL + "/api/user/workspace-sync/v2")
	require.NoError(t, err)
	defer response.Body.Close()
	require.Equal(t, http.StatusUnauthorized, response.StatusCode)
}

func TestWorkspaceV2UpgradeReturns403ForValidTokenWithoutWSScope(t *testing.T) {
	_, httpServer := newWorkspaceV2HTTPTestServer(t, func(*gin.Context) (*middleware.AuthenticatedUserToken, *code.Code) {
		return nil, code.ErrorAuthTokenScopeRestricted
	})
	req, err := http.NewRequest(http.MethodGet, httpServer.URL+"/api/user/workspace-sync/v2", nil)
	require.NoError(t, err)
	req.Header.Set("Authorization", "Bearer test-token")
	response, err := http.DefaultClient.Do(req)
	require.NoError(t, err)
	defer response.Body.Close()
	require.Equal(t, http.StatusForbidden, response.StatusCode)
}

func TestWorkspaceV2ProductionAuthenticationRejectsBadAuthorizationBeforeUpgrade(t *testing.T) {
	token := newWorkspaceV2AuthJWT(t)
	tests := []struct {
		name      string
		configure func(*http.Request)
	}{
		{name: "missing Authorization"},
		{
			name: "multiple Authorization values",
			configure: func(req *http.Request) {
				req.Header["Authorization"] = []string{"Bearer " + token, "Bearer " + token}
			},
		},
		{
			name: "malformed Authorization",
			configure: func(req *http.Request) {
				req.Header.Set("Authorization", "Bearer "+token+" extra")
			},
		},
		{
			name: "non Bearer Authorization",
			configure: func(req *http.Request) {
				req.Header.Set("Authorization", "Token "+token)
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			server, httpServer, _ := newWorkspaceV2ProductionAuthHTTPTestServer(t, "p:ws c:fns-agent f:workspace_rw")
			req, err := http.NewRequest(http.MethodGet, httpServer.URL+"/api/user/workspace-sync/v2", nil)
			require.NoError(t, err)
			req.Header.Set("X-Client", "fns-agent")
			if tt.configure != nil {
				tt.configure(req)
			}

			response, err := http.DefaultClient.Do(req)
			require.NoError(t, err)
			defer response.Body.Close()
			require.Equal(t, http.StatusUnauthorized, response.StatusCode)
			require.Empty(t, workspaceV2ConnectionSnapshot(server))
		})
	}
}

func TestWorkspaceV2RawNetworkTabAuthorizationIsRejectedBeforeUpgrade(t *testing.T) {
	server, httpServer, tokenService := newWorkspaceV2ProductionAuthHTTPTestServer(t, "p:ws c:fns-agent f:workspace_rw")
	address := strings.TrimPrefix(httpServer.URL, "http://")
	conn, err := net.DialTimeout("tcp", address, time.Second)
	require.NoError(t, err)
	defer conn.Close()

	request := "GET /api/user/workspace-sync/v2 HTTP/1.1\r\n" +
		"Host: " + address + "\r\n" +
		"Connection: Upgrade\r\n" +
		"Upgrade: websocket\r\n" +
		"Sec-WebSocket-Version: 13\r\n" +
		"Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n" +
		"X-Client: fns-agent\r\n" +
		"Authorization: Bearer\t" + newWorkspaceV2AuthJWT(t) + "\r\n\r\n"
	_, err = io.WriteString(conn, request)
	require.NoError(t, err)

	response, err := http.ReadResponse(bufio.NewReader(conn), &http.Request{Method: http.MethodGet})
	require.NoError(t, err)
	defer response.Body.Close()
	require.Equal(t, http.StatusUnauthorized, response.StatusCode)
	require.Zero(t, tokenService.lookupCalls)
	require.Empty(t, workspaceV2ConnectionSnapshot(server))
}

func TestWorkspaceV2ProductionAuthenticationRequiresExactAgentScope(t *testing.T) {
	tests := []struct {
		name          string
		scope         string
		requestClient string
		wantStatus    int
	}{
		{name: "exact workspace permission", scope: "p:ws c:fns-agent f:workspace_rw", wantStatus: http.StatusSwitchingProtocols},
		{name: "REST protocol", scope: "p:rest c:fns-agent f:workspace_rw", wantStatus: http.StatusForbidden},
		{name: "wrong client", scope: "p:ws c:other-agent f:workspace_rw", wantStatus: http.StatusForbidden},
		{name: "matching non-agent client", scope: "p:ws c:other-agent f:workspace_rw", requestClient: "other-agent", wantStatus: http.StatusForbidden},
		{name: "missing function", scope: "p:ws c:fns-agent", wantStatus: http.StatusForbidden},
		{name: "wrong function", scope: "p:ws c:fns-agent f:note_rw", wantStatus: http.StatusForbidden},
		{name: "blank scope", scope: "", wantStatus: http.StatusForbidden},
		{name: "wildcard function", scope: "p:ws c:fns-agent f:*", wantStatus: http.StatusForbidden},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			requestClient := tt.requestClient
			if requestClient == "" {
				requestClient = "fns-agent"
			}
			server, httpServer, tokenService := newWorkspaceV2ProductionAuthHTTPTestServer(t, tt.scope)
			events := &workspaceV2TestEvents{}
			conn, response, err := gws.NewClient(events, &gws.ClientOption{
				Addr:             "ws" + strings.TrimPrefix(httpServer.URL, "http") + "/api/user/workspace-sync/v2",
				HandshakeTimeout: 3 * time.Second,
				RequestHeader: http.Header{
					"Authorization": []string{"Bearer " + newWorkspaceV2AuthJWT(t)},
					"X-Client":      []string{requestClient},
				},
			})
			if response != nil && response.Body != nil {
				defer response.Body.Close()
			}
			if conn != nil {
				defer conn.NetConn().Close()
			}
			require.NotNil(t, response)
			require.Equal(t, tt.wantStatus, response.StatusCode)
			if tt.wantStatus == http.StatusSwitchingProtocols {
				require.NoError(t, err)
				require.NotNil(t, conn)
				require.Eventually(t, func() bool {
					return len(workspaceV2ConnectionSnapshot(server)) == 1
				}, time.Second, 5*time.Millisecond)
			} else {
				require.Error(t, err)
				require.Nil(t, conn)
				require.Empty(t, workspaceV2ConnectionSnapshot(server))
			}
			require.Equal(t, int64(41), tokenService.lookupUID)
			require.Equal(t, int64(7), tokenService.lookupID)
			require.Equal(t, 1, tokenService.lookupCalls)
		})
	}
}

func TestWorkspaceV2UpdateTokenScopeRequiresExactWorkspacePermission(t *testing.T) {
	tests := []struct {
		name      string
		scope     string
		wantClose bool
	}{
		{name: "exact workspace permission remains connected", scope: "p:ws c:fns-agent f:workspace_rw"},
		{name: "missing function", scope: "p:ws c:fns-agent", wantClose: true},
		{name: "wrong function", scope: "p:ws c:fns-agent f:note_rw", wantClose: true},
		{name: "wildcard function", scope: "p:ws c:fns-agent f:*", wantClose: true},
		{name: "wrong client", scope: "p:ws c:other-agent f:workspace_rw", wantClose: true},
		{name: "wrong protocol", scope: "p:rest c:fns-agent f:workspace_rw", wantClose: true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			server, httpServer := newWorkspaceV2HTTPTestServer(t, func(*gin.Context) (*middleware.AuthenticatedUserToken, *code.Code) {
				return workspaceV2TestIdentity(), nil
			})
			events := &workspaceV2TestEvents{closes: make(chan error, 1)}
			conn, response, err := gws.NewClient(events, &gws.ClientOption{
				Addr:             "ws" + strings.TrimPrefix(httpServer.URL, "http") + "/api/user/workspace-sync/v2",
				HandshakeTimeout: 3 * time.Second,
				RequestHeader:    http.Header{"Authorization": []string{"Bearer test-token"}},
			})
			require.NoError(t, err)
			require.Equal(t, http.StatusSwitchingProtocols, response.StatusCode)
			if response.Body != nil {
				response.Body.Close()
			}
			defer conn.NetConn().Close()
			go conn.ReadLoop()
			require.Eventually(t, func() bool {
				return len(workspaceV2ConnectionSnapshot(server)) == 1
			}, time.Second, 5*time.Millisecond)

			server.UpdateTokenScope(41, 7, tt.scope)
			if !tt.wantClose {
				select {
				case closeErr := <-events.closes:
					t.Fatalf("exact workspace permission closed connection: %v", closeErr)
				case <-time.After(50 * time.Millisecond):
				}
				require.Len(t, workspaceV2ConnectionSnapshot(server), 1)
				return
			}

			select {
			case received := <-events.closes:
				var closeErr *gws.CloseError
				require.True(t, errors.As(received, &closeErr), "close error: %v", received)
				require.Equal(t, uint16(1008), closeErr.Code)
				require.Equal(t, "token_scope_revoked", string(closeErr.Reason))
			case <-time.After(time.Second):
				t.Fatal("timed out waiting for token scope revocation close")
			}
		})
	}
}

func TestWorkspaceV2KickTokenClosesWithPolicyViolation(t *testing.T) {
	server, httpServer := newWorkspaceV2HTTPTestServer(t, func(*gin.Context) (*middleware.AuthenticatedUserToken, *code.Code) {
		return workspaceV2TestIdentity(), nil
	})
	events := &workspaceV2TestEvents{closes: make(chan error, 1)}
	conn, response, err := gws.NewClient(events, &gws.ClientOption{
		Addr:             "ws" + strings.TrimPrefix(httpServer.URL, "http") + "/api/user/workspace-sync/v2",
		HandshakeTimeout: 3 * time.Second,
		RequestHeader:    http.Header{"Authorization": []string{"Bearer test-token"}},
	})
	require.NoError(t, err)
	require.Equal(t, http.StatusSwitchingProtocols, response.StatusCode)
	if response.Body != nil {
		response.Body.Close()
	}
	defer conn.NetConn().Close()
	go conn.ReadLoop()
	require.Eventually(t, func() bool {
		return len(workspaceV2ConnectionSnapshot(server)) == 1
	}, time.Second, 5*time.Millisecond)

	server.KickToken(41, 7)
	select {
	case received := <-events.closes:
		var closeErr *gws.CloseError
		require.True(t, errors.As(received, &closeErr), "close error: %v", received)
		require.Equal(t, uint16(1008), closeErr.Code)
		require.Equal(t, "token_revoked", string(closeErr.Reason))
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for token revocation close")
	}
}

func TestWorkspaceV2ConcurrentScopeLossAndTokenKickCloseOnce(t *testing.T) {
	server, httpServer := newWorkspaceV2HTTPTestServer(t, func(*gin.Context) (*middleware.AuthenticatedUserToken, *code.Code) {
		return workspaceV2TestIdentity(), nil
	})
	events := &workspaceV2TestEvents{closes: make(chan error, 2)}
	conn, response, err := gws.NewClient(events, &gws.ClientOption{
		Addr:             "ws" + strings.TrimPrefix(httpServer.URL, "http") + "/api/user/workspace-sync/v2",
		HandshakeTimeout: 3 * time.Second,
		RequestHeader:    http.Header{"Authorization": []string{"Bearer test-token"}},
	})
	require.NoError(t, err)
	require.Equal(t, http.StatusSwitchingProtocols, response.StatusCode)
	if response.Body != nil {
		response.Body.Close()
	}
	defer conn.NetConn().Close()
	go conn.ReadLoop()
	require.Eventually(t, func() bool {
		return len(workspaceV2ConnectionSnapshot(server)) == 1
	}, time.Second, 5*time.Millisecond)

	var invalidations sync.WaitGroup
	invalidations.Add(2)
	go func() {
		defer invalidations.Done()
		server.UpdateTokenScope(41, 7, "p:ws c:fns-agent")
	}()
	go func() {
		defer invalidations.Done()
		server.KickToken(41, 7)
	}()
	invalidations.Wait()

	select {
	case received := <-events.closes:
		var closeErr *gws.CloseError
		require.True(t, errors.As(received, &closeErr), "close error: %v", received)
		require.Equal(t, uint16(1008), closeErr.Code)
		require.Contains(t, []string{"token_scope_revoked", "token_revoked"}, string(closeErr.Reason))
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for concurrent token invalidation close")
	}
	require.Eventually(t, func() bool {
		return len(workspaceV2ConnectionSnapshot(server)) == 0
	}, time.Second, 5*time.Millisecond)
}

func TestWorkspaceV2UpgradeBindsUIDTokenAndClientBeforeReadLoop(t *testing.T) {
	server, httpServer := newWorkspaceV2HTTPTestServer(t, func(*gin.Context) (*middleware.AuthenticatedUserToken, *code.Code) {
		return workspaceV2TestIdentity(), nil
	})

	events := &workspaceV2TestEvents{}
	conn, response, err := gws.NewClient(events, &gws.ClientOption{
		Addr:             "ws" + strings.TrimPrefix(httpServer.URL, "http") + "/api/user/workspace-sync/v2",
		HandshakeTimeout: 3 * time.Second,
		RequestHeader:    http.Header{"Authorization": []string{"Bearer test-token"}},
	})
	require.NoError(t, err)
	require.Equal(t, http.StatusSwitchingProtocols, response.StatusCode)
	if response.Body != nil {
		response.Body.Close()
	}
	t.Cleanup(func() {
		_ = conn.NetConn().Close()
		server.Close()
		server.WaitAllClosed(time.Second)
	})
	go conn.ReadLoop()

	require.Eventually(t, func() bool {
		server.mu.RLock()
		defer server.mu.RUnlock()
		for _, candidate := range server.connections {
			return candidate.uid == 41 && candidate.tokenID == 7 && candidate.clientType == "fns-agent" && candidate.clientName == "desktop"
		}
		return false
	}, time.Second, 5*time.Millisecond)
}

func TestWorkspaceV2WaitAllClosedWaitsForConnectionLoops(t *testing.T) {
	server := &WorkspaceV2Server{
		closed:       true,
		janitorDone:  true,
		activeLoops:  1,
		shutdownDone: make(chan struct{}),
	}
	finished := make(chan struct{})
	go func() {
		require.NoError(t, server.WaitAllClosed(time.Second))
		close(finished)
	}()
	select {
	case <-finished:
		t.Fatal("WaitAllClosed returned before connection loops stopped")
	case <-time.After(25 * time.Millisecond):
	}

	server.finishConnectionLoop()
	select {
	case <-finished:
	case <-time.After(time.Second):
		t.Fatal("WaitAllClosed did not observe stopped connection loops")
	}
}

func TestWorkspaceV2WaitAllClosedWaitsForTransferJanitor(t *testing.T) {
	server := NewWorkspaceV2Server(nil, nil, gws.ServerOption{})
	managerLocked := true
	server.transfers.mu.Lock()
	defer func() {
		if managerLocked {
			server.transfers.mu.Unlock()
		}
		server.Close()
		server.WaitAllClosed(time.Second)
	}()

	require.Eventually(t, func() bool {
		stack := make([]byte, 1<<20)
		stack = stack[:runtime.Stack(stack, true)]
		return strings.Contains(string(stack), "workspaceV2TransferManager).Expire")
	}, 2*time.Second, 10*time.Millisecond, "transfer janitor did not reach the real manager expiry path")

	server.Close()
	finished := make(chan struct{})
	go func() {
		server.WaitAllClosed(time.Second)
		close(finished)
	}()
	select {
	case <-finished:
		t.Fatal("WaitAllClosed returned before the transfer janitor exited")
	case <-time.After(25 * time.Millisecond):
	}

	server.transfers.mu.Unlock()
	managerLocked = false
	select {
	case <-finished:
	case <-time.After(time.Second):
		t.Fatal("WaitAllClosed did not observe the transfer janitor exit")
	}
}

func TestWorkspaceV2CloseJoinsJanitorRepeatedly(t *testing.T) {
	for iteration := range 100 {
		server := NewWorkspaceV2Server(nil, nil, gws.ServerOption{})
		server.Close()
		require.NoErrorf(t, server.WaitAllClosed(time.Second), "iteration %d", iteration)
		require.NoErrorf(t, server.WaitAllClosed(time.Second), "repeat wait iteration %d", iteration)
		server.mu.RLock()
		require.True(t, server.janitorDone)
		require.True(t, server.shutdownComplete)
		require.Zero(t, server.activeHandlers)
		require.Zero(t, server.activeLoops)
		server.mu.RUnlock()
	}
}

func TestWorkspaceV2WaitAllClosedReturnsObservableTimeout(t *testing.T) {
	server := NewWorkspaceV2Server(nil, nil, gws.ServerOption{})
	managerLocked := true
	server.transfers.mu.Lock()
	t.Cleanup(func() {
		if managerLocked {
			server.transfers.mu.Unlock()
		}
		server.Close()
		_ = server.WaitAllClosed(time.Second)
	})

	require.Eventually(t, func() bool {
		return workspaceV2RuntimeStackContains("workspaceV2TransferManager).Expire")
	}, 2*time.Second, 10*time.Millisecond, "transfer janitor did not reach the real manager expiry path")
	server.Close()

	err := server.WaitAllClosed(5 * time.Millisecond)
	var timeoutErr *WorkspaceV2ShutdownTimeoutError
	require.ErrorAs(t, err, &timeoutErr)
	require.Equal(t, 5*time.Millisecond, timeoutErr.Timeout)
	require.True(t, timeoutErr.Closed)
	require.True(t, timeoutErr.JanitorRunning)
	require.Zero(t, timeoutErr.PendingConnections)
	require.Zero(t, timeoutErr.PendingHandlers)
	require.Zero(t, timeoutErr.PendingLoops)

	server.transfers.mu.Unlock()
	managerLocked = false
	require.NoError(t, server.WaitAllClosed(time.Second))
	require.NoError(t, server.WaitAllClosed(time.Second), "completed waits must be repeatable")
}

func TestWorkspaceV2RepeatedWaitTimeoutsDoNotStrandWaiters(t *testing.T) {
	server := NewWorkspaceV2Server(nil, nil, gws.ServerOption{})
	managerLocked := true
	server.transfers.mu.Lock()
	t.Cleanup(func() {
		if managerLocked {
			server.transfers.mu.Unlock()
		}
		server.Close()
		_ = server.WaitAllClosed(time.Second)
	})

	require.Eventually(t, func() bool {
		return workspaceV2RuntimeStackContains("workspaceV2TransferManager).Expire")
	}, 2*time.Second, 10*time.Millisecond, "transfer janitor did not reach the real manager expiry path")
	server.Close()
	before := runtime.NumGoroutine()
	for range 20 {
		var timeoutErr *WorkspaceV2ShutdownTimeoutError
		require.ErrorAs(t, server.WaitAllClosed(time.Millisecond), &timeoutErr)
		require.True(t, timeoutErr.JanitorRunning)
	}
	after := runtime.NumGoroutine()
	require.LessOrEqual(t, after, before+2,
		"timed-out waits must not leave one uncancelable waiter goroutine per call")

	server.transfers.mu.Unlock()
	managerLocked = false
	require.NoError(t, server.WaitAllClosed(time.Second))
}

func TestWorkspaceV2CloseRejectsUpgradeWhoseAuthenticationWasInFlight(t *testing.T) {
	gin.SetMode(gin.TestMode)
	server := NewWorkspaceV2Server(nil, NewWorkspaceV2AccessPolicy(configForWorkspaceV2Test(canonicalTempDir(t))), gws.ServerOption{})
	authEntered := make(chan struct{})
	releaseAuth := make(chan struct{})
	server.authenticate = func(*gin.Context) (*middleware.AuthenticatedUserToken, *code.Code) {
		close(authEntered)
		<-releaseAuth
		return workspaceV2TestIdentity(), nil
	}
	router := gin.New()
	router.GET("/api/user/workspace-sync/v2", server.Run())
	httpServer := httptest.NewServer(router)
	t.Cleanup(func() {
		httpServer.Close()
		server.Close()
		_ = server.WaitAllClosed(time.Second)
	})

	type clientResult struct {
		conn     *gws.Conn
		response *http.Response
		err      error
	}
	resultCh := make(chan clientResult, 1)
	go func() {
		conn, response, err := gws.NewClient(&workspaceV2TestEvents{}, &gws.ClientOption{
			Addr:             "ws" + strings.TrimPrefix(httpServer.URL, "http") + "/api/user/workspace-sync/v2",
			HandshakeTimeout: 3 * time.Second,
			RequestHeader:    http.Header{"Authorization": []string{"Bearer test-token"}},
		})
		resultCh <- clientResult{conn: conn, response: response, err: err}
	}()

	select {
	case <-authEntered:
	case <-time.After(time.Second):
		t.Fatal("authentication did not enter")
	}
	server.Close()
	var timeoutErr *WorkspaceV2ShutdownTimeoutError
	require.ErrorAs(t, server.WaitAllClosed(5*time.Millisecond), &timeoutErr)
	require.Equal(t, 1, timeoutErr.PendingHandlers)
	require.Error(t, server.ctx.Err())
	close(releaseAuth)

	select {
	case result := <-resultCh:
		if result.response != nil && result.response.Body != nil {
			defer result.response.Body.Close()
		}
		if result.conn != nil {
			defer result.conn.NetConn().Close()
		}
		require.Error(t, result.err)
		require.Nil(t, result.conn)
		if result.response != nil {
			require.NotEqual(t, http.StatusSwitchingProtocols, result.response.StatusCode)
		}
		require.Empty(t, workspaceV2ConnectionSnapshot(server))
		require.NoError(t, server.WaitAllClosed(time.Second))
		require.NoError(t, server.WaitAllClosed(time.Second), "completed waits must be repeatable")
	case <-time.After(4 * time.Second):
		t.Fatal("websocket client did not return")
	}
}

func TestWorkspaceV2CloseWaitsForUpgradedConnectionPublicationBoundary(t *testing.T) {
	gin.SetMode(gin.TestMode)
	server := NewWorkspaceV2Server(nil, NewWorkspaceV2AccessPolicy(configForWorkspaceV2Test(canonicalTempDir(t))), gws.ServerOption{})
	server.authenticate = func(*gin.Context) (*middleware.AuthenticatedUserToken, *code.Code) {
		return workspaceV2TestIdentity(), nil
	}
	upgraded := make(chan struct{})
	releaseUpgrade := make(chan struct{})
	var releaseOnce sync.Once
	release := func() { releaseOnce.Do(func() { close(releaseUpgrade) }) }
	realUpgrade := server.upgrade
	server.upgrade = func(w http.ResponseWriter, r *http.Request) (*gws.Conn, error) {
		conn, err := realUpgrade(w, r)
		if err == nil {
			close(upgraded)
			<-releaseUpgrade
		}
		return conn, err
	}
	router := gin.New()
	router.GET("/api/user/workspace-sync/v2", server.Run())
	httpServer := httptest.NewServer(router)
	t.Cleanup(func() {
		release()
		server.Close()
		_ = server.WaitAllClosed(time.Second)
		httpServer.Close()
	})

	events := &workspaceV2TestEvents{closes: make(chan error, 1)}
	type clientResult struct {
		conn     *gws.Conn
		response *http.Response
		err      error
	}
	resultCh := make(chan clientResult, 1)
	go func() {
		conn, response, err := gws.NewClient(events, &gws.ClientOption{
			Addr:             "ws" + strings.TrimPrefix(httpServer.URL, "http") + "/api/user/workspace-sync/v2",
			HandshakeTimeout: 3 * time.Second,
			RequestHeader:    http.Header{"Authorization": []string{"Bearer test-token"}},
		})
		resultCh <- clientResult{conn: conn, response: response, err: err}
	}()

	select {
	case <-upgraded:
	case <-time.After(time.Second):
		t.Fatal("websocket upgrade did not cross the handshake boundary")
	}
	server.Close()
	var timeoutErr *WorkspaceV2ShutdownTimeoutError
	require.ErrorAs(t, server.WaitAllClosed(5*time.Millisecond), &timeoutErr)
	require.Equal(t, 1, timeoutErr.PendingHandlers)
	require.Zero(t, timeoutErr.PendingConnections)
	require.Zero(t, timeoutErr.PendingLoops)
	release()

	var client clientResult
	select {
	case client = <-resultCh:
	case <-time.After(time.Second):
		t.Fatal("websocket client did not return after publication was released")
	}
	require.NoError(t, client.err)
	require.NotNil(t, client.conn)
	defer client.conn.NetConn().Close()
	require.NotNil(t, client.response)
	require.Equal(t, http.StatusSwitchingProtocols, client.response.StatusCode)
	if client.response.Body != nil {
		defer client.response.Body.Close()
	}
	go client.conn.ReadLoop()
	select {
	case received := <-events.closes:
		var closeErr *gws.CloseError
		require.True(t, errors.As(received, &closeErr), "post-Close upgraded connection error: %v", received)
		require.Equal(t, uint16(1001), closeErr.Code)
		require.Equal(t, "server_shutdown", string(closeErr.Reason))
	case <-time.After(time.Second):
		t.Fatal("post-Close upgraded connection was not closed")
	}
	require.NoError(t, server.WaitAllClosed(time.Second))
	require.Empty(t, workspaceV2ConnectionSnapshot(server))
	server.mu.RLock()
	require.True(t, server.closed)
	require.True(t, server.janitorDone)
	require.Zero(t, server.activeHandlers)
	require.Zero(t, server.activeLoops)
	require.True(t, server.shutdownComplete)
	server.mu.RUnlock()
}

func TestWorkspaceV2PostCloseRegistrationIsRejected(t *testing.T) {
	server := NewWorkspaceV2Server(nil, nil, gws.ServerOption{})
	server.Close()
	require.NoError(t, server.WaitAllClosed(time.Second))

	conn := &gws.Conn{}
	connection := newWorkspaceV2Connection(server, conn, workspaceV2ConnectionIdentity{uid: 41, tokenID: 7})
	require.False(t, server.registerConnection(connection))
	require.Error(t, connection.ctx.Err(), "post-Close connection context must inherit server cancellation")
	require.Empty(t, workspaceV2ConnectionSnapshot(server))
	server.mu.RLock()
	require.Zero(t, server.activeHandlers)
	require.Zero(t, server.activeLoops)
	server.mu.RUnlock()
	require.NoError(t, server.WaitAllClosed(time.Second))
}

func workspaceV2RuntimeStackContains(fragment string) bool {
	return workspaceV2RuntimeStackCount(fragment) > 0
}

func workspaceV2RuntimeStackCount(fragment string) int {
	stack := make([]byte, 1<<20)
	stack = stack[:runtime.Stack(stack, true)]
	return strings.Count(string(stack), fragment)
}

type workspaceV2TestEvents struct {
	gws.BuiltinEventHandler
	messages chan []byte
	closes   chan error
}

func (e *workspaceV2TestEvents) OnMessage(_ *gws.Conn, message *gws.Message) {
	defer message.Close()
	if e.messages != nil {
		e.messages <- append([]byte(nil), message.Data.Bytes()...)
	}
}

func (e *workspaceV2TestEvents) OnClose(_ *gws.Conn, err error) {
	if e.closes != nil {
		e.closes <- err
	}
}

func newWorkspaceV2HTTPTestServer(t *testing.T, authenticate workspaceV2Authenticator) (*WorkspaceV2Server, *httptest.Server) {
	t.Helper()
	gin.SetMode(gin.TestMode)
	server := NewWorkspaceV2Server(nil, NewWorkspaceV2AccessPolicy(configForWorkspaceV2Test(canonicalTempDir(t))), gws.ServerOption{})
	server.authenticate = authenticate
	router := gin.New()
	router.GET("/api/user/workspace-sync/v2", server.Run())
	httpServer := httptest.NewServer(router)
	t.Cleanup(func() {
		server.Close()
		server.WaitAllClosed(time.Second)
		httpServer.Close()
	})
	return server, httpServer
}

type workspaceV2AuthTokenService struct {
	service.TokenService
	activeToken *domain.AuthToken
	activeErr   error
	lookupUID   int64
	lookupID    int64
	lookupCalls int
}

func (s *workspaceV2AuthTokenService) GetActiveToken(_ context.Context, uid int64, tokenID int64) (*domain.AuthToken, error) {
	s.lookupUID = uid
	s.lookupID = tokenID
	s.lookupCalls++
	return s.activeToken, s.activeErr
}

func newWorkspaceV2ProductionAuthHTTPTestServer(t *testing.T, scope string) (*WorkspaceV2Server, *httptest.Server, *workspaceV2AuthTokenService) {
	t.Helper()
	tokenService := &workspaceV2AuthTokenService{activeToken: &domain.AuthToken{
		ID:          7,
		UID:         41,
		TokenString: "workspace-v2-nonce",
		Scope:       scope,
		ClientType:  "fns-agent",
		Status:      1,
		ExpiredAt:   time.Now().Add(time.Hour),
		IssueType:   2,
	}}
	testApp := internalapp.NewTestApp(&internalapp.Services{TokenService: tokenService})
	testApp.Config().Security.AuthTokenKey = workspaceV2AuthTestSecret
	gin.SetMode(gin.TestMode)
	server := NewWorkspaceV2Server(testApp, NewWorkspaceV2AccessPolicy(configForWorkspaceV2Test(canonicalTempDir(t))), gws.ServerOption{})
	router := gin.New()
	router.GET("/api/user/workspace-sync/v2", server.Run())
	httpServer := httptest.NewServer(router)
	t.Cleanup(func() {
		server.Close()
		server.WaitAllClosed(time.Second)
		httpServer.Close()
	})
	return server, httpServer, tokenService
}

func newWorkspaceV2AuthJWT(t *testing.T) string {
	t.Helper()
	token, err := pkgapp.NewTokenManager(pkgapp.TokenConfig{
		SecretKey: workspaceV2AuthTestSecret,
		Expiry:    time.Hour,
	}).Generate(41, "", "", 7, "workspace-v2-nonce")
	require.NoError(t, err)
	return token
}

func workspaceV2ConnectionSnapshot(server *WorkspaceV2Server) []*workspaceV2Connection {
	server.mu.RLock()
	defer server.mu.RUnlock()
	connections := make([]*workspaceV2Connection, 0, len(server.connections))
	for _, connection := range server.connections {
		connections = append(connections, connection)
	}
	return connections
}

func workspaceV2TestIdentity() *middleware.AuthenticatedUserToken {
	return &middleware.AuthenticatedUserToken{
		User:  &pkgapp.UserEntity{UID: 41, TokenID: 7},
		Scope: "p:ws c:fns-agent f:workspace_rw", ClientType: "fns-agent", ClientName: "desktop", ClientVersion: "2.0",
	}
}

func configForWorkspaceV2Test(root string) config.WorkspaceConfig {
	return config.WorkspaceConfig{
		MaxWorkspacesPerUser: config.WorkspaceMaxPerUser,
		Roots: []config.WorkspaceRootConfig{{
			UID:         41,
			WorkspaceID: workspaceV2SecurityWorkspaceID,
			Root:        root,
		}},
	}
}
