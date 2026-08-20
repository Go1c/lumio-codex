package websocket_router

import (
	"context"
	"fmt"
	"net/http"
	"sync"
	"time"

	"github.com/gin-gonic/gin"
	"github.com/haierkeys/fast-note-sync-service/internal/app"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/haierkeys/fast-note-sync-service/internal/middleware"
	"github.com/haierkeys/fast-note-sync-service/internal/service"
	pkgapp "github.com/haierkeys/fast-note-sync-service/pkg/app"
	"github.com/haierkeys/fast-note-sync-service/pkg/code"
	"github.com/lxzan/gws"
	"go.uber.org/zap"
)

type workspaceV2Authenticator func(*gin.Context) (*middleware.AuthenticatedUserToken, *code.Code)
type workspaceV2UpgradeFunc func(http.ResponseWriter, *http.Request) (*gws.Conn, error)

const (
	workspaceV2AuthProtocol = "ws"
	workspaceV2AuthClient   = "fns-agent"
	workspaceV2AuthFunction = "workspace_rw"

	workspaceV2ShutdownWriteTimeout = 100 * time.Millisecond
)

type WorkspaceV2Server struct {
	gws.BuiltinEventHandler
	app                *app.App
	syncService        service.WorkspaceSyncService
	blobStore          service.WorkspaceBlobStore
	access             *WorkspaceV2AccessPolicy
	upgrade            workspaceV2UpgradeFunc
	ctx                context.Context
	cancel             context.CancelFunc
	version            string
	mu                 sync.RWMutex
	connections        map[*gws.Conn]*workspaceV2Connection
	closed             bool
	janitorDone        bool
	activeHandlers     int
	activeLoops        int
	activeTransfers    int
	shutdownDone       chan struct{}
	shutdownComplete   bool
	hub                *workspaceV2Hub
	transfers          *workspaceV2TransferManager
	completedTransfers workspaceV2CompletedTransferRegistry
	authenticate       workspaceV2Authenticator
	logger             *zap.Logger
}

func NewWorkspaceV2Server(appContainer *app.App, access *WorkspaceV2AccessPolicy, option gws.ServerOption) *WorkspaceV2Server {
	ctx, cancel := context.WithCancel(context.Background())
	server := &WorkspaceV2Server{
		app:          appContainer,
		access:       access,
		ctx:          ctx,
		cancel:       cancel,
		connections:  make(map[*gws.Conn]*workspaceV2Connection),
		shutdownDone: make(chan struct{}),
		hub:          &workspaceV2Hub{},
		transfers:    &workspaceV2TransferManager{},
		logger:       zap.NewNop(),
	}
	server.version = "dev"
	if appContainer != nil && appContainer.Version().Version != "" {
		server.version = appContainer.Version().Version
	}
	if appContainer != nil && appContainer.Logger() != nil {
		server.logger = appContainer.Logger()
	}
	if appContainer != nil && appContainer.Services != nil {
		server.syncService = appContainer.WorkspaceSyncService
		server.blobStore = appContainer.WorkspaceBlobStore
	}
	option.ParallelEnabled = false
	option.CheckUtf8Enabled = false
	option.ReadMaxPayloadSize = dto.WorkspaceBlobHeaderSize + dto.WorkspaceBlobChunkSize
	option.WriteMaxPayloadSize = dto.WorkspaceBlobHeaderSize + dto.WorkspaceBlobChunkSize
	option.PermessageDeflate.Enabled = false
	option.Recovery = gws.Recovery
	server.upgrade = gws.NewUpgrader(server, &option).Upgrade
	go func() {
		defer server.finishJanitor()
		server.transferJanitor()
	}()
	server.authenticate = func(c *gin.Context) (*middleware.AuthenticatedUserToken, *code.Code) {
		if server.app == nil || server.app.Config() == nil || server.app.TokenService == nil {
			return nil, code.ErrorNotUserAuthToken
		}
		return middleware.AuthenticateBearerUserTokenForProtocol(
			c,
			server.app.Config().Security.AuthTokenKey,
			server.app.TokenService,
			workspaceV2AuthProtocol,
			workspaceV2AuthClient,
			workspaceV2AuthFunction,
		)
	}
	return server
}

func (s *WorkspaceV2Server) transferJanitor() {
	ticker := time.NewTicker(time.Second)
	defer ticker.Stop()
	for {
		select {
		case <-s.ctx.Done():
			return
		case now := <-ticker.C:
			s.transfers.Expire(now)
			s.completedTransfers.Expire(now)
		}
	}
}

func (s *WorkspaceV2Server) Run() gin.HandlerFunc {
	return func(c *gin.Context) {
		if s == nil || s.upgrade == nil || !s.beginHandler() {
			c.AbortWithStatus(http.StatusServiceUnavailable)
			return
		}
		defer s.finishHandler()
		identity, appErr := s.authenticate(c)
		if appErr != nil || identity == nil || identity.User == nil {
			status := http.StatusUnauthorized
			if appErr != nil && workspaceV2AuthHTTPStatus(appErr) == http.StatusForbidden {
				status = http.StatusForbidden
			}
			c.AbortWithStatus(status)
			return
		}
		if s.isClosed() {
			c.AbortWithStatus(http.StatusServiceUnavailable)
			return
		}
		conn, err := s.upgrade(c.Writer, c.Request)
		if err != nil {
			return
		}
		connection := newWorkspaceV2Connection(s, conn, workspaceV2ConnectionIdentity{
			uid:           identity.User.UID,
			tokenID:       identity.User.TokenID,
			scope:         identity.Scope,
			clientType:    identity.ClientType,
			clientName:    identity.ClientName,
			clientVersion: identity.ClientVersion,
		})
		if !s.registerConnection(connection) {
			_ = conn.WriteClose(1001, []byte("server_shutdown"))
			return
		}
		connection.start()
	}
}

func (s *WorkspaceV2Server) beginHandler() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.closed {
		return false
	}
	s.activeHandlers++
	return true
}

func (s *WorkspaceV2Server) finishHandler() {
	s.mu.Lock()
	if s.activeHandlers > 0 {
		s.activeHandlers--
	}
	s.completeShutdownLocked()
	s.mu.Unlock()
}

func (s *WorkspaceV2Server) isClosed() bool {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.closed
}

func (s *WorkspaceV2Server) registerConnection(connection *workspaceV2Connection) bool {
	if connection == nil || connection.conn == nil {
		return false
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.closed {
		return false
	}
	s.connections[connection.conn] = connection
	s.activeLoops += workspaceV2ConnectionLoopCount
	return true
}

func (s *WorkspaceV2Server) finishConnectionLoop() {
	s.mu.Lock()
	if s.activeLoops > 0 {
		s.activeLoops--
	}
	s.completeShutdownLocked()
	s.mu.Unlock()
}

func (s *WorkspaceV2Server) finishTransfer() {
	s.mu.Lock()
	if s.activeTransfers > 0 {
		s.activeTransfers--
	}
	s.completeShutdownLocked()
	s.mu.Unlock()
}

func (s *WorkspaceV2Server) finishJanitor() {
	s.mu.Lock()
	s.janitorDone = true
	s.completeShutdownLocked()
	s.mu.Unlock()
}

func (s *WorkspaceV2Server) completeShutdownLocked() {
	if s.shutdownComplete || !s.closed || !s.janitorDone || s.activeHandlers != 0 || s.activeLoops != 0 ||
		s.activeTransfers != 0 || len(s.connections) != 0 {
		return
	}
	if s.shutdownDone == nil {
		s.shutdownDone = make(chan struct{})
	}
	close(s.shutdownDone)
	s.shutdownComplete = true
}

func workspaceV2AuthHTTPStatus(appErr *code.Code) int {
	if appErr == nil {
		return http.StatusUnauthorized
	}
	switch appErr.Code() {
	case code.ErrorAuthTokenClientRestricted.Code(), code.ErrorAuthTokenScopeRestricted.Code(),
		code.ErrorAuthTokenUARestricted.Code(), code.ErrorAuthTokenIPRestricted.Code():
		return http.StatusForbidden
	default:
		return http.StatusUnauthorized
	}
}

func (s *WorkspaceV2Server) removeConnection(connection *workspaceV2Connection) {
	if s == nil || connection == nil {
		return
	}
	s.mu.Lock()
	if current, ok := s.connections[connection.conn]; ok && current == connection {
		delete(s.connections, connection.conn)
		s.completeShutdownLocked()
	}
	s.mu.Unlock()
}

func (s *WorkspaceV2Server) OnOpen(socket *gws.Conn) {
	if connection := s.connection(socket); connection != nil {
		_ = socket.SetReadDeadline(time.Now().Add(workspaceV2HeartbeatWait))
		_ = connection
	}
}

func (s *WorkspaceV2Server) OnClose(socket *gws.Conn, _ error) {
	if connection := s.connection(socket); connection != nil {
		connection.cleanup()
	}
}

func (s *WorkspaceV2Server) OnPing(socket *gws.Conn, payload []byte) {
	if connection := s.connection(socket); connection != nil {
		_ = socket.SetReadDeadline(time.Now().Add(workspaceV2HeartbeatWait))
		_ = connection.send(gws.OpcodePong, payload)
	}
}

func (s *WorkspaceV2Server) OnPong(socket *gws.Conn, _ []byte) {
	if s.connection(socket) != nil {
		_ = socket.SetReadDeadline(time.Now().Add(workspaceV2HeartbeatWait))
	}
}

func (s *WorkspaceV2Server) OnMessage(socket *gws.Conn, message *gws.Message) {
	if message == nil {
		return
	}
	data := append([]byte(nil), message.Data.Bytes()...)
	opcode := message.Opcode
	_ = message.Close()
	if connection := s.connection(socket); connection != nil {
		connection.enqueueInbound(workspaceV2InboundFrame{opcode: opcode, data: data})
	}
}

func (s *WorkspaceV2Server) connection(socket *gws.Conn) *workspaceV2Connection {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.connections[socket]
}

func (s *WorkspaceV2Server) CloseAllConnections() {
	if s == nil {
		return
	}
	s.mu.RLock()
	connections := make([]*workspaceV2Connection, 0, len(s.connections))
	for _, connection := range s.connections {
		connections = append(connections, connection)
	}
	s.mu.RUnlock()
	closeWorkspaceV2Connections(connections, 1001, "server_shutdown")
}

func closeWorkspaceV2Connections(connections []*workspaceV2Connection, code uint16, reason string) {
	writeDeadline := time.Now().Add(workspaceV2ShutdownWriteTimeout)
	for _, connection := range connections {
		if connection == nil || connection.conn == nil || connection.conn.NetConn() == nil {
			continue
		}
		if err := connection.conn.NetConn().SetWriteDeadline(writeDeadline); err != nil {
			_ = connection.conn.NetConn().Close()
		}
	}
	for _, connection := range connections {
		if connection != nil {
			connection.closeWithCode(code, reason)
		}
	}
}

// WorkspaceV2ShutdownTimeoutError reports the server-owned work that did not
// finish within the caller's bounded shutdown window.
type WorkspaceV2ShutdownTimeoutError struct {
	Timeout            time.Duration
	Closed             bool
	PendingConnections int
	PendingHandlers    int
	PendingLoops       int
	PendingTransfers   int
	JanitorRunning     bool
}

func (e *WorkspaceV2ShutdownTimeoutError) Error() string {
	return fmt.Sprintf(
		"workspace v2 shutdown timed out after %s (closed=%t connections=%d handlers=%d loops=%d transfers=%d janitorRunning=%t)",
		e.Timeout, e.Closed, e.PendingConnections, e.PendingHandlers, e.PendingLoops, e.PendingTransfers, e.JanitorRunning,
	)
}

func (s *WorkspaceV2Server) WaitAllClosed(timeout time.Duration) error {
	if s == nil {
		return nil
	}
	s.mu.Lock()
	if s.shutdownDone == nil {
		s.shutdownDone = make(chan struct{})
	}
	done := s.shutdownDone
	s.mu.Unlock()
	if timeout <= 0 {
		select {
		case <-done:
			return nil
		default:
			return s.shutdownTimeoutError(timeout)
		}
	}
	timer := time.NewTimer(timeout)
	defer timer.Stop()
	select {
	case <-done:
		return nil
	case <-timer.C:
		select {
		case <-done:
			return nil
		default:
			return s.shutdownTimeoutError(timeout)
		}
	}
}

func (s *WorkspaceV2Server) shutdownTimeoutError(timeout time.Duration) error {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return &WorkspaceV2ShutdownTimeoutError{
		Timeout:            timeout,
		Closed:             s.closed,
		PendingConnections: len(s.connections),
		PendingHandlers:    s.activeHandlers,
		PendingLoops:       s.activeLoops,
		PendingTransfers:   s.activeTransfers,
		JanitorRunning:     !s.janitorDone,
	}
}

func (s *WorkspaceV2Server) UpdateTokenScope(uid, tokenID int64, scope string) {
	if s == nil {
		return
	}
	s.mu.RLock()
	connections := make([]*workspaceV2Connection, 0)
	for _, connection := range s.connections {
		if connection.uid == uid && connection.tokenID == tokenID {
			connections = append(connections, connection)
		}
	}
	s.mu.RUnlock()
	for _, connection := range connections {
		connection.stateMu.Lock()
		connection.scope = scope
		authorized := pkgapp.VerifyExactPermissions(
			scope,
			workspaceV2AuthProtocol,
			workspaceV2AuthClient,
			workspaceV2AuthFunction,
		) && connection.clientType == workspaceV2AuthClient
		connection.stateMu.Unlock()
		if !authorized {
			connection.closeWithCode(1008, "token_scope_revoked")
		}
	}
}

func (s *WorkspaceV2Server) KickToken(uid, tokenID int64) {
	if s == nil {
		return
	}
	s.mu.RLock()
	connections := make([]*workspaceV2Connection, 0)
	for _, connection := range s.connections {
		if connection.uid == uid && connection.tokenID == tokenID {
			connections = append(connections, connection)
		}
	}
	s.mu.RUnlock()
	for _, connection := range connections {
		connection.closeWithCode(1008, "token_revoked")
	}
}

func (s *WorkspaceV2Server) Close() {
	if s == nil {
		return
	}
	s.mu.Lock()
	s.closed = true
	connections := make([]*workspaceV2Connection, 0, len(s.connections))
	for _, connection := range s.connections {
		connections = append(connections, connection)
	}
	s.completeShutdownLocked()
	s.mu.Unlock()
	if s.cancel != nil {
		s.cancel()
	}
	closeWorkspaceV2Connections(connections, 1001, "server_shutdown")
}

var _ gws.Event = (*WorkspaceV2Server)(nil)
