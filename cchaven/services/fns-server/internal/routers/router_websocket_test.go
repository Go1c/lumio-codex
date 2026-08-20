package routers

import (
	"context"
	"encoding/binary"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/gin-gonic/gin"
	"github.com/go-playground/locales/en"
	ut "github.com/go-playground/universal-translator"
	internalapp "github.com/haierkeys/fast-note-sync-service/internal/app"
	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	v1 "github.com/haierkeys/fast-note-sync-service/internal/proto/v1"
	"github.com/haierkeys/fast-note-sync-service/internal/routers/websocket_router"
	svcmocks "github.com/haierkeys/fast-note-sync-service/internal/service/mocks"
	pkgapp "github.com/haierkeys/fast-note-sync-service/pkg/app"
	"github.com/haierkeys/fast-note-sync-service/pkg/code"
	"github.com/lxzan/gws"
	"github.com/stretchr/testify/mock"
	"github.com/stretchr/testify/require"
	"google.golang.org/protobuf/proto"
)

const productionWiringTokenKey = "production-wiring-regression-secret"

type productionWiringApp struct {
	*internalapp.App
}

func (a productionWiringApp) SubmitTaskAsync(ctx context.Context, task func(context.Context) error) error {
	return task(ctx)
}

type productionWiringFrame struct {
	opcode gws.Opcode
	data   []byte
}

type productionWiringClientEvents struct {
	gws.BuiltinEventHandler
	frames chan productionWiringFrame
}

func (e *productionWiringClientEvents) OnMessage(_ *gws.Conn, message *gws.Message) {
	defer message.Close()
	e.frames <- productionWiringFrame{opcode: message.Opcode, data: append([]byte(nil), message.Data.Bytes()...)}
}

type productionWiringResponse struct {
	action  string
	payload []byte
	Code    int             `json:"code"`
	Status  bool            `json:"status"`
	Data    json.RawMessage `json:"data"`
	Vault   string          `json:"vault"`
	Context string          `json:"context"`
}

type productionWiringHarness struct {
	t      *testing.T
	wss    *pkgapp.WebsocketServer
	server *httptest.Server
	conn   *gws.Conn
	frames chan productionWiringFrame
}

func newProductionWiringHarness(t *testing.T, testApp *internalapp.App) *productionWiringHarness {
	t.Helper()
	gin.SetMode(gin.TestMode)
	tracingEnabled := false
	testApp.Repositories = &internalapp.Repositories{}
	testApp.Config().Tracer.Enabled = &tracingEnabled
	testApp.Config().App.DefaultContextTimeout = 60
	testApp.Config().Security.AuthTokenKey = productionWiringTokenKey

	wss := pkgapp.NewWebsocketServer(pkgapp.WSConfig{}, productionWiringApp{App: testApp})
	initWebSocketRoutes(wss, testApp)
	wss.UseTokenVerify(func(context.Context, int64, int64, string, string, string, string, string, string) (string, string, error) {
		return "p:ws c:* f:*", "", nil
	})
	wss.UseUserVerify(func(_ *pkgapp.WebsocketClient, uid int64) (*pkgapp.UserSelectEntity, error) {
		return &pkgapp.UserSelectEntity{UID: uid, Nickname: "production-wiring-user"}, nil
	})

	engine := gin.New()
	translator := ut.New(en.New(), en.New())
	registerAPIRoutes(engine, testApp, wss, translator)
	server := httptest.NewServer(engine)
	events := &productionWiringClientEvents{frames: make(chan productionWiringFrame, 16)}
	conn, response, err := gws.NewClient(events, &gws.ClientOption{
		Addr:             "ws" + strings.TrimPrefix(server.URL, "http") + "/api/user/sync?protocol=protobuf",
		HandshakeTimeout: 3 * time.Second,
	})
	if response != nil && response.Body != nil {
		defer response.Body.Close()
	}
	require.NoError(t, err)

	h := &productionWiringHarness{t: t, wss: wss, server: server, conn: conn, frames: events.frames}
	go conn.ReadLoop()
	t.Cleanup(h.close)
	return h
}

func (h *productionWiringHarness) close() {
	_ = h.conn.WriteClose(1000, []byte("test complete"))
	_ = h.conn.NetConn().Close()
	h.wss.CloseAllConnections()
	h.server.Close()
	h.wss.WaitAllClosed(time.Second)
}

func (h *productionWiringHarness) sendText(payload string) {
	h.t.Helper()
	require.NoError(h.t, h.conn.WriteString(payload))
}

func (h *productionWiringHarness) sendBinary(payload []byte) {
	h.t.Helper()
	require.NoError(h.t, h.conn.WriteMessage(gws.OpcodeBinary, payload))
}

func (h *productionWiringHarness) receiveFrame() productionWiringFrame {
	h.t.Helper()
	select {
	case frame := <-h.frames:
		return frame
	case <-time.After(3 * time.Second):
		h.t.Fatal("timed out waiting for production-wired websocket response")
		return productionWiringFrame{}
	}
}

func (h *productionWiringHarness) receiveText() productionWiringResponse {
	h.t.Helper()
	frame := h.receiveFrame()
	require.Equal(h.t, gws.OpcodeText, frame.opcode)
	response := productionWiringResponse{payload: frame.data}
	body := frame.data
	if index := strings.IndexByte(string(body), '|'); index >= 0 {
		response.action = string(body[:index])
		body = body[index+1:]
	}
	require.NoError(h.t, json.Unmarshal(body, &response))
	return response
}

func (h *productionWiringHarness) receiveProtobuf() (*v1.WSMessage, *v1.WSResponse) {
	h.t.Helper()
	frame := h.receiveFrame()
	require.Equal(h.t, gws.OpcodeBinary, frame.opcode)
	require.True(h.t, len(frame.data) >= 2)
	require.Equal(h.t, []byte("pb"), frame.data[:2])
	var envelope v1.WSMessage
	require.NoError(h.t, proto.Unmarshal(frame.data[2:], &envelope))
	var response v1.WSResponse
	require.NoError(h.t, proto.Unmarshal(envelope.Data, &response))
	return &envelope, &response
}

func productionWiringProtobufFrame(t *testing.T, action string, payload proto.Message) []byte {
	t.Helper()
	inner, err := proto.Marshal(payload)
	require.NoError(t, err)
	outer, err := proto.Marshal(&v1.WSMessage{Type: action, Data: inner})
	require.NoError(t, err)
	return append([]byte("pb"), outer...)
}

func TestRegisterAPIRoutesKeepsV1WebSocketGET(t *testing.T) {
	gin.SetMode(gin.TestMode)
	tracingEnabled := false
	testApp := internalapp.NewTestApp(&internalapp.Services{})
	testApp.Repositories = &internalapp.Repositories{}
	testApp.Config().Tracer.Enabled = &tracingEnabled
	wss := pkgapp.NewWebsocketServer(pkgapp.WSConfig{}, testApp)
	engine := gin.New()

	registerAPIRoutes(engine, testApp, wss, nil)

	count := 0
	for _, route := range engine.Routes() {
		if route.Method == http.MethodGet && route.Path == "/api/user/sync" {
			count++
		}
	}
	if count != 1 {
		t.Fatalf("GET /api/user/sync route count = %d, want exactly 1", count)
	}
}

func TestRegisterAPIRoutesRegistersWorkspaceV2WhenProvided(t *testing.T) {
	gin.SetMode(gin.TestMode)
	tracingEnabled := false
	testApp := internalapp.NewTestApp(&internalapp.Services{})
	testApp.Repositories = &internalapp.Repositories{}
	testApp.Config().Tracer.Enabled = &tracingEnabled
	wss := pkgapp.NewWebsocketServer(pkgapp.WSConfig{}, testApp)
	workspaceV2 := websocket_router.NewWorkspaceV2Server(testApp, nil, gws.ServerOption{})
	t.Cleanup(func() {
		workspaceV2.Close()
		workspaceV2.WaitAllClosed(time.Second)
	})
	engine := gin.New()

	registerAPIRoutes(engine, testApp, wss, nil, workspaceV2)

	count := 0
	for _, route := range engine.Routes() {
		if route.Method == http.MethodGet && route.Path == "/api/user/workspace-sync/v2" {
			count++
		}
	}
	require.Equal(t, 1, count)
}

func TestLegacyWebSocketVerifierDoesNotPrintCredentialValues(t *testing.T) {
	source, err := os.ReadFile("router_websocket.go")
	require.NoError(t, err)
	for _, forbidden := range []string{
		"[WSDebug]",
		"req_nonce=",
		"db_nonce=",
		"scope=%s",
		"User-Agent mismatch: req=",
		"IP mismatch: req=",
	} {
		require.NotContains(t, string(source), forbidden)
	}
}

func TestProductionWebSocketWiringPreservesV1SyncAndBinaryDispatch(t *testing.T) {
	noteSvc := new(svcmocks.MockNoteService)
	fileSvc := new(svcmocks.MockFileService)
	vaultSvc := new(svcmocks.MockVaultService)
	testApp := internalapp.NewTestApp(&internalapp.Services{
		NoteService: noteSvc, FileService: fileSvc, VaultService: vaultSvc,
	})

	vaultSvc.On("GetOrCreate", mock.Anything, int64(7), "main").
		Return(&domain.Vault{ID: 1, UID: 7, Name: "main"}, nil).Twice()
	fileSvc.On("ListByLastTime", mock.Anything, int64(7), mock.MatchedBy(func(request *dto.FileSyncRequest) bool {
		return request.Context == "file-context" && request.Vault == "main" && request.LastTime == 0 && len(request.Files) == 1 && request.Files[0].PathHash == "file-hash"
	})).Return([]*dto.FileDTO{{
		Action: "delete", Path: "files/old.bin", PathHash: "file-hash", ContentHash: "file-content-hash",
		Size: 99, Ctime: 40, Mtime: 50, UpdatedTimestamp: 60,
	}}, nil).Once()
	noteSvc.On("WithClient", "obsidian", "wiring-test", "1.0.0").Return(noteSvc).Once()
	noteSvc.On("ListByLastTime", mock.Anything, int64(7), mock.MatchedBy(func(request *dto.NoteSyncRequest) bool {
		return request.Context == "note-context" && request.Vault == "main" && request.LastTime == 0
	})).Return([]*dto.NoteDTO{{
		Path: "notes/a.md", PathHash: "note-hash", Content: "alpha|beta", ContentHash: "note-content-hash",
		Ctime: 10, Mtime: 20, UpdatedTimestamp: 30,
	}}, nil).Once()

	h := newProductionWiringHarness(t, testApp)

	h.sendText(`NoteSync|{"vault":"main"}`)
	preAuth := h.receiveText()
	require.Empty(t, preAuth.action)
	require.Equal(t, code.ErrorNotUserAuthToken.Code(), preAuth.Code)
	require.False(t, preAuth.Status)

	token, err := pkgapp.NewTokenManager(pkgapp.TokenConfig{SecretKey: productionWiringTokenKey}).Generate(7, "production-wiring-user", "", 13, "nonce")
	require.NoError(t, err)
	h.sendText("Authorization|" + token)
	authorized := h.receiveText()
	require.Equal(t, "Authorization", authorized.action)
	require.Equal(t, code.Success.Code(), authorized.Code)
	require.True(t, authorized.Status)

	h.sendText(`FileSync|{"context":"file-context","vault":"main","lastTime":0,"files":[{"path":"files/old.bin","pathHash":"file-hash","contentHash":"client-hash","mtime":1}],"delFiles":[],"missingFiles":[]}`)
	fileEnd := h.receiveText()
	require.Equal(t, websocket_router.FileSyncEnd, fileEnd.action)
	require.Equal(t, code.Success.Code(), fileEnd.Code)
	require.True(t, fileEnd.Status)
	require.Equal(t, "main", fileEnd.Vault)
	require.Equal(t, "file-context", fileEnd.Context)
	var fileEndData dto.FileSyncEndMessage
	require.NoError(t, json.Unmarshal(fileEnd.Data, &fileEndData))
	require.Equal(t, int64(1), fileEndData.NeedDeleteCount)

	fileDelete := h.receiveText()
	require.Equal(t, websocket_router.FileSyncDelete, fileDelete.action)
	var fileDeleteData dto.FileSyncDeleteMessage
	require.NoError(t, json.Unmarshal(fileDelete.Data, &fileDeleteData))
	require.Equal(t, "files/old.bin", fileDeleteData.Path)
	require.Equal(t, "file-hash", fileDeleteData.PathHash)
	require.Equal(t, int64(99), fileDeleteData.Size)

	h.sendText(`ClientInfo|{"name":"wiring-test","version":"1.0.0","type":"obsidian","isDesktop":true,"protobuf":true}`)
	clientInfoEnvelope, clientInfoResponse := h.receiveProtobuf()
	require.Equal(t, "ClientInfo", clientInfoEnvelope.Type)
	require.Equal(t, int32(code.Success.Code()), clientInfoResponse.Code)
	require.True(t, clientInfoResponse.Status)

	h.sendBinary(productionWiringProtobufFrame(t, websocket_router.NoteReceiveSync, &v1.NoteSyncRequest{
		Context: "note-context", Vault: "main", LastTime: 0,
	}))
	noteEndEnvelope, noteEndResponse := h.receiveProtobuf()
	require.Equal(t, websocket_router.NoteSyncEnd, noteEndEnvelope.Type)
	require.Equal(t, int32(code.Success.Code()), noteEndResponse.Code)
	require.True(t, noteEndResponse.Status)
	require.Equal(t, "main", noteEndResponse.Vault)
	require.Equal(t, "note-context", noteEndResponse.Context)
	var noteEnd v1.NoteSyncEndMessage
	require.NoError(t, proto.Unmarshal(noteEndResponse.Data, &noteEnd))
	require.Equal(t, int64(1), noteEnd.NeedModifyCount)

	noteModifyEnvelope, noteModifyResponse := h.receiveProtobuf()
	require.Equal(t, websocket_router.NoteSyncModify, noteModifyEnvelope.Type)
	require.Equal(t, int32(code.Success.Code()), noteModifyResponse.Code)
	var noteModify v1.NoteSyncModifyMessage
	require.NoError(t, proto.Unmarshal(noteModifyResponse.Data, &noteModify))
	require.Equal(t, "notes/a.md", noteModify.Path)
	require.Equal(t, "note-hash", noteModify.PathHash)
	require.Equal(t, "alpha|beta", noteModify.Content)

	sessionID := "10000000-0000-4000-8000-000000000099"
	binaryFrame := make([]byte, 2+36+4)
	copy(binaryFrame[:2], websocket_router.VaultFileMsgType)
	copy(binaryFrame[2:38], sessionID)
	binary.BigEndian.PutUint32(binaryFrame[38:42], 0)
	h.sendBinary(binaryFrame)
	binaryResult := h.receiveText()
	require.Empty(t, binaryResult.action)
	require.Equal(t, code.ErrorFileUploadSessionNotFound.Code(), binaryResult.Code)
	var binaryData struct {
		SessionID string `json:"sessionID"`
	}
	require.NoError(t, json.Unmarshal(binaryResult.Data, &binaryData))
	require.Equal(t, sessionID, binaryData.SessionID, "production binary registration must strip the 00 prefix")

	noteSvc.AssertExpectations(t)
	fileSvc.AssertExpectations(t)
	vaultSvc.AssertExpectations(t)
}
