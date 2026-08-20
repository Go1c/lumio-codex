package websocket_router

import (
	"context"
	"errors"
	"io"
	"net"
	"net/http"
	"runtime"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/gin-gonic/gin"
	"github.com/google/uuid"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/haierkeys/fast-note-sync-service/internal/middleware"
	"github.com/haierkeys/fast-note-sync-service/pkg/code"
	"github.com/lxzan/gws"
	"github.com/stretchr/testify/require"
)

type workspaceV2PipeListener struct {
	connections chan net.Conn
	closed      chan struct{}
	closeOnce   sync.Once
}

func newWorkspaceV2PipeListener(capacity int) *workspaceV2PipeListener {
	return &workspaceV2PipeListener{
		connections: make(chan net.Conn, capacity),
		closed:      make(chan struct{}),
	}
}

func (l *workspaceV2PipeListener) Accept() (net.Conn, error) {
	select {
	case connection := <-l.connections:
		return connection, nil
	case <-l.closed:
		return nil, net.ErrClosed
	}
}

func (l *workspaceV2PipeListener) Close() error {
	l.closeOnce.Do(func() { close(l.closed) })
	return nil
}

func (l *workspaceV2PipeListener) Addr() net.Addr {
	return workspaceV2PipeAddr("workspace-v2-pipe")
}

type workspaceV2PipeAddr string

func (a workspaceV2PipeAddr) Network() string { return "pipe" }
func (a workspaceV2PipeAddr) String() string  { return string(a) }

type workspaceV2ObservedPipeConn struct {
	net.Conn
	armed         atomic.Bool
	writeStarted  chan struct{}
	writeOnce     sync.Once
	closeObserved chan struct{}
	closeOnce     sync.Once
}

func newWorkspaceV2ObservedPipeConn(connection net.Conn) *workspaceV2ObservedPipeConn {
	return &workspaceV2ObservedPipeConn{
		Conn:          connection,
		writeStarted:  make(chan struct{}),
		closeObserved: make(chan struct{}),
	}
}

func (c *workspaceV2ObservedPipeConn) Write(payload []byte) (int, error) {
	if c.armed.Load() {
		c.writeOnce.Do(func() { close(c.writeStarted) })
	}
	return c.Conn.Write(payload)
}

func (c *workspaceV2ObservedPipeConn) Close() error {
	c.closeOnce.Do(func() { close(c.closeObserved) })
	return c.Conn.Close()
}

func TestWorkspaceV2ServerCloseInterruptsConcurrentStalledWrites(t *testing.T) {
	const connectionCount = 6

	gin.SetMode(gin.TestMode)
	server := NewWorkspaceV2Server(nil, nil, gws.ServerOption{})
	server.authenticate = func(*gin.Context) (*middleware.AuthenticatedUserToken, *code.Code) {
		return workspaceV2TestIdentity(), nil
	}
	router := gin.New()
	router.GET("/api/user/workspace-sync/v2", server.Run())
	listener := newWorkspaceV2PipeListener(connectionCount)
	httpServer := &http.Server{Handler: router}
	serveDone := make(chan error, 1)
	serveJoined := false
	go func() { serveDone <- httpServer.Serve(listener) }()

	clients := make([]*gws.Conn, 0, connectionCount)
	serverPipes := make([]*workspaceV2ObservedPipeConn, 0, connectionCount)
	closePeers := func() {
		for _, client := range clients {
			if client != nil && client.NetConn() != nil {
				_ = client.NetConn().Close()
			}
		}
	}
	t.Cleanup(func() {
		closePeers()
		server.Close()
		_ = server.WaitAllClosed(time.Second)
		_ = httpServer.Close()
		_ = listener.Close()
		if !serveJoined {
			select {
			case <-serveDone:
			case <-time.After(time.Second):
				t.Error("pipe HTTP server did not stop")
			}
		}
	})

	for range connectionCount {
		serverPipe, clientPipe := net.Pipe()
		observed := newWorkspaceV2ObservedPipeConn(serverPipe)
		serverPipes = append(serverPipes, observed)
		listener.connections <- observed
		client, response, err := gws.NewClientFromConn(
			gws.BuiltinEventHandler{},
			&gws.ClientOption{
				Addr:             "ws://workspace-v2.test/api/user/workspace-sync/v2",
				HandshakeTimeout: time.Second,
			},
			clientPipe,
		)
		require.NoError(t, err)
		require.Equal(t, http.StatusSwitchingProtocols, response.StatusCode)
		clients = append(clients, client)
	}

	require.Eventually(t, func() bool {
		return len(workspaceV2ConnectionSnapshot(server)) == connectionCount
	}, time.Second, time.Millisecond, "production route did not publish every upgraded connection")

	blockedWriterStacksBefore := workspaceV2RuntimeStackCount("github.com/lxzan/gws/internal.WriteN")
	writeResults := make([]chan error, 0, connectionCount)
	for _, connection := range workspaceV2ConnectionSnapshot(server) {
		connectionPipe, ok := connection.conn.NetConn().(*workspaceV2ObservedPipeConn)
		require.True(t, ok)
		connectionPipe.armed.Store(true)
		writeResult := make(chan error, 1)
		writeResults = append(writeResults, writeResult)
		go func() {
			writeResult <- connection.conn.WriteMessage(gws.OpcodeBinary, []byte("stalled server write"))
		}()
	}
	for _, connectionPipe := range serverPipes {
		select {
		case <-connectionPipe.writeStarted:
		case <-time.After(time.Second):
			t.Fatal("server WriteMessage did not reach the unread net.Pipe peer")
		}
	}

	closeStarted := time.Now()
	closeDone := make(chan struct{})
	go func() {
		server.Close()
		close(closeDone)
	}()
	select {
	case <-closeDone:
	case <-time.After(500 * time.Millisecond):
		for _, connectionPipe := range serverPipes {
			select {
			case <-connectionPipe.closeObserved:
				t.Error("underlying connection closed before the stalled close path was released")
			default:
			}
		}
		closePeers()
		select {
		case <-closeDone:
		case <-time.After(time.Second):
			t.Fatal("Close remained blocked after the unread peers were closed")
		}
		t.Fatal("Close blocked behind concurrent WriteMessage calls before cleanup could close their NetConns")
	}
	require.Less(t, time.Since(closeStarted), 500*time.Millisecond,
		"Close accumulated a separate write wait for each stalled connection")

	for _, writeResult := range writeResults {
		select {
		case err := <-writeResult:
			require.Error(t, err)
		case <-time.After(time.Second):
			t.Fatal("stalled WriteMessage goroutine did not stop")
		}
	}
	for _, connectionPipe := range serverPipes {
		select {
		case <-connectionPipe.closeObserved:
		case <-time.After(time.Second):
			t.Fatal("server shutdown did not close an underlying connection")
		}
	}
	require.NoError(t, server.WaitAllClosed(time.Second))
	require.Empty(t, workspaceV2ConnectionSnapshot(server))
	require.Eventually(t, func() bool {
		return workspaceV2RuntimeStackCount("github.com/lxzan/gws/internal.WriteN") <= blockedWriterStacksBefore
	}, time.Second, time.Millisecond, "stalled gws writer goroutine leaked after shutdown")

	require.NoError(t, httpServer.Close())
	require.NoError(t, listener.Close())
	select {
	case err := <-serveDone:
		serveJoined = true
		require.True(t, errors.Is(err, http.ErrServerClosed) || errors.Is(err, net.ErrClosed), err)
	case <-time.After(time.Second):
		t.Fatal("pipe HTTP server did not stop after shutdown")
	}
}

type workspaceV2ContextIgnoringStore struct {
	workspaceV2BlobStoreStub
	started     chan struct{}
	release     chan struct{}
	startedOnce sync.Once
	releaseOnce sync.Once
}

type workspaceV2ContextIgnoringNonReadingStore struct {
	workspaceV2ContextIgnoringStore
}

func newWorkspaceV2ContextIgnoringNonReadingStore() *workspaceV2ContextIgnoringNonReadingStore {
	store := &workspaceV2ContextIgnoringNonReadingStore{}
	store.started = make(chan struct{})
	store.release = make(chan struct{})
	return store
}

func (s *workspaceV2ContextIgnoringNonReadingStore) Put(
	_ context.Context,
	_ int64,
	_ dto.WorkspaceContentHash,
	_ uint64,
	source io.Reader,
) error {
	s.startedOnce.Do(func() { close(s.started) })
	<-s.release
	_, err := io.ReadAll(source)
	return err
}

func newWorkspaceV2ContextIgnoringStore() *workspaceV2ContextIgnoringStore {
	return &workspaceV2ContextIgnoringStore{
		started: make(chan struct{}),
		release: make(chan struct{}),
	}
}

func (s *workspaceV2ContextIgnoringStore) Put(
	_ context.Context,
	_ int64,
	_ dto.WorkspaceContentHash,
	_ uint64,
	source io.Reader,
) error {
	if _, err := io.ReadAll(source); err != nil {
		return err
	}
	s.startedOnce.Do(func() { close(s.started) })
	<-s.release
	return nil
}

func (s *workspaceV2ContextIgnoringStore) releasePut() {
	s.releaseOnce.Do(func() { close(s.release) })
}

func TestWorkspaceV2CloseReturnsBeforeContextIgnoringUploadSettles(t *testing.T) {
	store := newWorkspaceV2ContextIgnoringStore()
	t.Cleanup(store.releasePut)
	server, conn, events, owner := workspaceV2PrepareBlobSession(t, store)
	payload := []byte("durable but delayed")
	begin := workspaceV2UploadBegin("60000000-0000-4000-8000-000000000299", payload)
	transferID := uuid.MustParse(string(begin.TransferID))

	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000299", begin)
	workspaceV2RequireBlobBeginSuccess(t, workspaceV2Receive(t, events), "10000000-0000-4000-8000-000000000299", begin)
	workspaceV2SendUploadChunk(t, conn, transferID, payload)
	select {
	case <-store.started:
	case <-time.After(time.Second):
		t.Fatal("context-ignoring Put did not reach its delayed commit return")
	}

	closeDone := make(chan struct{})
	go func() {
		server.Close()
		close(closeDone)
	}()
	select {
	case <-closeDone:
	case <-time.After(100 * time.Millisecond):
		store.releasePut()
		<-closeDone
		t.Fatal("Close blocked on the context-ignoring Put")
	}

	var timeoutErr *WorkspaceV2ShutdownTimeoutError
	err := server.WaitAllClosed(10 * time.Millisecond)
	require.ErrorAs(t, err, &timeoutErr)
	require.True(t, timeoutErr.Closed)
	require.Equal(t, 1, timeoutErr.PendingConnections)
	require.Equal(t, 1, timeoutErr.PendingTransfers)
	require.ErrorContains(t, err, "transfers=1")
	workspaceV2RequireExactTransferSlots(t, server, owner, owner.workspaceV2Transfer(transferID))

	store.releasePut()
	require.NoError(t, server.WaitAllClosed(time.Second))
	workspaceV2RequireCompletedReceipt(t, owner, workspaceV2EndForBegin(begin))
	workspaceV2RequireExactTransferSlots(t, server, owner, nil)
}

func TestWorkspaceV2RepeatedWaitTimeoutsRetainDelayedUploadOwnership(t *testing.T) {
	store := newWorkspaceV2ContextIgnoringStore()
	t.Cleanup(store.releasePut)
	server, conn, events, _ := workspaceV2PrepareBlobSession(t, store)
	payload := []byte("owned until release")
	begin := workspaceV2UploadBegin("60000000-0000-4000-8000-000000000298", payload)

	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000298", begin)
	workspaceV2RequireBlobBeginSuccess(t, workspaceV2Receive(t, events), "10000000-0000-4000-8000-000000000298", begin)
	workspaceV2SendUploadChunk(t, conn, uuid.MustParse(string(begin.TransferID)), payload)
	select {
	case <-store.started:
	case <-time.After(time.Second):
		t.Fatal("context-ignoring Put did not start")
	}

	closeDone := make(chan struct{})
	go func() {
		server.Close()
		close(closeDone)
	}()
	select {
	case <-closeDone:
	case <-time.After(100 * time.Millisecond):
		store.releasePut()
		<-closeDone
		t.Fatal("Close blocked on the context-ignoring Put")
	}
	before := runtime.NumGoroutine()
	for range 20 {
		var timeoutErr *WorkspaceV2ShutdownTimeoutError
		err := server.WaitAllClosed(time.Millisecond)
		require.ErrorAs(t, err, &timeoutErr)
		require.Equal(t, 1, timeoutErr.PendingConnections)
		require.Equal(t, 1, timeoutErr.PendingTransfers)
		require.ErrorContains(t, err, "transfers=1")
	}
	after := runtime.NumGoroutine()
	require.LessOrEqual(t, after, before+2)

	store.releasePut()
	require.NoError(t, server.WaitAllClosed(time.Second))
}

func TestWorkspaceV2CloseReturnsWhileUploadPipeWriteIsBlocked(t *testing.T) {
	store := newWorkspaceV2ContextIgnoringNonReadingStore()
	t.Cleanup(store.releasePut)
	server, conn, events, _ := workspaceV2PrepareBlobSession(t, store)
	payload := []byte("blocked pipe write")
	begin := workspaceV2UploadBegin("60000000-0000-4000-8000-000000000294", payload)

	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000294", begin)
	workspaceV2RequireBlobBeginSuccess(t, workspaceV2Receive(t, events), "10000000-0000-4000-8000-000000000294", begin)
	select {
	case <-store.started:
	case <-time.After(time.Second):
		t.Fatal("context-ignoring Put did not start")
	}
	workspaceV2SendUploadChunk(t, conn, uuid.MustParse(string(begin.TransferID)), payload)
	require.Eventually(t, func() bool {
		return workspaceV2RuntimeStackContains("io.(*pipe).write")
	}, time.Second, time.Millisecond, "upload processor did not block in the pipe write")

	closeDone := make(chan struct{})
	go func() {
		server.Close()
		close(closeDone)
	}()
	select {
	case <-closeDone:
	case <-time.After(100 * time.Millisecond):
		store.releasePut()
		<-closeDone
		t.Fatal("Close blocked behind the upload pipe writer")
	}

	var timeoutErr *WorkspaceV2ShutdownTimeoutError
	require.ErrorAs(t, server.WaitAllClosed(10*time.Millisecond), &timeoutErr)
	require.Equal(t, 1, timeoutErr.PendingTransfers)
	store.releasePut()
	require.NoError(t, server.WaitAllClosed(time.Second))
}

func TestWorkspaceV2ConcurrentCloseAndCleanupRemainPromptAndIdempotent(t *testing.T) {
	store := newWorkspaceV2ContextIgnoringStore()
	t.Cleanup(store.releasePut)
	server, conn, events, owner := workspaceV2PrepareBlobSession(t, store)
	payload := []byte("concurrent cleanup")
	begin := workspaceV2UploadBegin("60000000-0000-4000-8000-000000000297", payload)

	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000297", begin)
	workspaceV2RequireBlobBeginSuccess(t, workspaceV2Receive(t, events), "10000000-0000-4000-8000-000000000297", begin)
	workspaceV2SendUploadChunk(t, conn, uuid.MustParse(string(begin.TransferID)), payload)
	select {
	case <-store.started:
	case <-time.After(time.Second):
		t.Fatal("context-ignoring Put did not start")
	}

	var callers sync.WaitGroup
	for index := range 32 {
		callers.Add(1)
		go func(index int) {
			defer callers.Done()
			switch index % 3 {
			case 0:
				server.Close()
			case 1:
				owner.cleanup()
			default:
				owner.closeWithCode(1001, "server_shutdown")
			}
		}(index)
	}
	callersDone := make(chan struct{})
	go func() {
		callers.Wait()
		close(callersDone)
	}()
	select {
	case <-callersDone:
	case <-time.After(200 * time.Millisecond):
		store.releasePut()
		<-callersDone
		t.Fatal("concurrent Close/cleanup calls blocked on the delayed Put")
	}

	var timeoutErr *WorkspaceV2ShutdownTimeoutError
	require.ErrorAs(t, server.WaitAllClosed(10*time.Millisecond), &timeoutErr)
	require.Equal(t, 1, timeoutErr.PendingConnections)
	store.releasePut()
	require.NoError(t, server.WaitAllClosed(time.Second))
}

func TestWorkspaceV2HelloStatePublishedBeforeSuccessWrite(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	writeStarted := make(chan struct{})
	releaseWrite := make(chan struct{})
	var releaseOnce sync.Once
	release := func() { releaseOnce.Do(func() { close(releaseWrite) }) }
	t.Cleanup(release)
	connection := &workspaceV2Connection{
		server:   &WorkspaceV2Server{version: "test"},
		conn:     &gws.Conn{},
		ctx:      ctx,
		cancel:   cancel,
		outbound: make(chan workspaceV2OutboundFrame, 1),
		writeMessage: func(gws.Opcode, []byte) error {
			close(writeStarted)
			<-releaseWrite
			return nil
		},
	}
	writerDone := make(chan struct{})
	go func() {
		connection.writerLoop()
		close(writerDone)
	}()
	clientID := dto.WorkspaceUUID("10000000-0000-4000-8000-000000000296")
	helloDone := make(chan error, 1)
	go func() {
		helloDone <- connection.handleHello(
			dto.WorkspaceUUID("10000000-0000-4000-8000-000000000295"),
			&dto.WorkspaceHelloRequest{
				ProtocolVersion: "2",
				ClientID:        clientID,
				ClientVersion:   "test",
				Capabilities:    []string{"binary_chunks", "conflicts", "snapshot_v1"},
			},
		)
	}()
	select {
	case <-writeStarted:
	case <-time.After(time.Second):
		t.Fatal("Hello success did not reach the external write boundary")
	}

	connection.stateMu.RLock()
	published := connection.helloDone
	publishedClientID := connection.helloClientID
	connection.stateMu.RUnlock()
	require.True(t, published, "Hello state must be published before success can be externally observed")
	require.Equal(t, clientID, publishedClientID)

	release()
	require.NoError(t, <-helloDone)
	cancel()
	select {
	case <-writerDone:
	case <-time.After(time.Second):
		t.Fatal("writer loop did not stop")
	}
}
