package websocket_router

import (
	"encoding/json"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/gin-gonic/gin"
	internalapp "github.com/haierkeys/fast-note-sync-service/internal/app"
	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	svcmocks "github.com/haierkeys/fast-note-sync-service/internal/service/mocks"
	pkgapp "github.com/haierkeys/fast-note-sync-service/pkg/app"
	"github.com/haierkeys/fast-note-sync-service/pkg/code"
	"github.com/lxzan/gws"
	"github.com/stretchr/testify/mock"
)

const v1RouterTokenKey = "v1-router-regression-secret"

type v1RouterClientEvents struct {
	gws.BuiltinEventHandler
	messages chan []byte
}

func (e *v1RouterClientEvents) OnMessage(_ *gws.Conn, message *gws.Message) {
	defer message.Close()
	payload := append([]byte(nil), message.Data.Bytes()...)
	select {
	case e.messages <- payload:
	default:
	}
}

type v1RouterResponse struct {
	action  string
	payload []byte
	Code    int             `json:"code"`
	Status  bool            `json:"status"`
	Message any             `json:"message"`
	Data    json.RawMessage `json:"data"`
	Details any             `json:"details"`
	Vault   string          `json:"vault"`
	Context string          `json:"context"`
}

type v1RouterHarness struct {
	t      *testing.T
	wss    *pkgapp.WebsocketServer
	server *httptest.Server
	conn   *gws.Conn
	events *v1RouterClientEvents
}

func newV1RouterHarness(t *testing.T, testApp *internalapp.App, authorize bool) *v1RouterHarness {
	t.Helper()
	gin.SetMode(gin.TestMode)
	testApp.Config().Security.AuthTokenKey = v1RouterTokenKey

	wss := pkgapp.NewWebsocketServer(pkgapp.WSConfig{}, testApp)
	wss.Use(NoteReceiveSync, NewNoteWSHandler(testApp).NoteSync)
	wss.Use(FileReceiveSync, NewFileWSHandler(testApp).FileSync)
	wss.UseInterceptor(NewMessageInterceptor(testApp))
	wss.UseUserVerify(func(_ *pkgapp.WebsocketClient, uid int64) (*pkgapp.UserSelectEntity, error) {
		return &pkgapp.UserSelectEntity{UID: uid, Nickname: "v1-router-user"}, nil
	})

	router := gin.New()
	router.GET("/api/user/sync", wss.Run())
	server := httptest.NewServer(router)
	events := &v1RouterClientEvents{messages: make(chan []byte, 8)}
	conn, response, err := gws.NewClient(events, &gws.ClientOption{
		Addr:             "ws" + strings.TrimPrefix(server.URL, "http") + "/api/user/sync",
		HandshakeTimeout: 3 * time.Second,
	})
	if response != nil && response.Body != nil {
		defer response.Body.Close()
	}
	if err != nil {
		server.Close()
		t.Fatalf("connect websocket: %v", err)
	}

	h := &v1RouterHarness{t: t, wss: wss, server: server, conn: conn, events: events}
	go conn.ReadLoop()
	t.Cleanup(h.close)
	if authorize {
		h.authorize()
	}
	return h
}

func (h *v1RouterHarness) close() {
	_ = h.conn.WriteClose(1000, []byte("test complete"))
	_ = h.conn.NetConn().Close()
	h.wss.CloseAllConnections()
	h.server.Close()
	h.wss.WaitAllClosed(time.Second)
}

func (h *v1RouterHarness) send(payload string) {
	h.t.Helper()
	if err := h.conn.WriteString(payload); err != nil {
		h.t.Fatalf("send websocket message: %v", err)
	}
}

func (h *v1RouterHarness) receive() v1RouterResponse {
	h.t.Helper()
	select {
	case raw := <-h.events.messages:
		response := v1RouterResponse{payload: raw}
		body := raw
		if index := strings.IndexByte(string(raw), '|'); index >= 0 {
			response.action = string(raw[:index])
			body = raw[index+1:]
		}
		if err := json.Unmarshal(body, &response); err != nil {
			h.t.Fatalf("decode websocket response %q: %v", raw, err)
		}
		return response
	case <-time.After(3 * time.Second):
		h.t.Fatal("timed out waiting for websocket response")
		return v1RouterResponse{}
	}
}

func (h *v1RouterHarness) authorize() {
	h.t.Helper()
	token, err := pkgapp.NewTokenManager(pkgapp.TokenConfig{SecretKey: v1RouterTokenKey}).Generate(7, "v1-router-user", "", 13, "nonce")
	if err != nil {
		h.t.Fatalf("generate JWT: %v", err)
	}
	h.send("Authorization|" + token)
	response := h.receive()
	if response.action != "Authorization" || response.Code != code.Success.Code() || !response.Status {
		h.t.Fatalf("authorize test client: action=%q code=%d status=%v payload=%s", response.action, response.Code, response.Status, response.payload)
	}
}

func assertV1SyncEnd(t *testing.T, response v1RouterResponse, action, context string, upload, modify, syncMtime, deleteCount int64) {
	t.Helper()
	if response.action != action || response.Code != code.Success.Code() || !response.Status {
		t.Fatalf("unexpected %s envelope: action=%q code=%d status=%v payload=%s", action, response.action, response.Code, response.Status, response.payload)
	}
	if response.Vault != "main" || response.Context != context {
		t.Fatalf("unexpected %s routing fields: vault=%q context=%q", action, response.Vault, response.Context)
	}
	var data struct {
		LastTime           int64 `json:"lastTime"`
		NeedUploadCount    int64 `json:"needUploadCount"`
		NeedModifyCount    int64 `json:"needModifyCount"`
		NeedSyncMtimeCount int64 `json:"needSyncMtimeCount"`
		NeedDeleteCount    int64 `json:"needDeleteCount"`
	}
	if err := json.Unmarshal(response.Data, &data); err != nil {
		t.Fatalf("decode %s data: %v", action, err)
	}
	if data.LastTime <= 0 || data.NeedUploadCount != upload || data.NeedModifyCount != modify || data.NeedSyncMtimeCount != syncMtime || data.NeedDeleteCount != deleteCount {
		t.Fatalf("unexpected %s counts: %+v", action, data)
	}
}

func assertV1MalformedSyncError(t *testing.T, response v1RouterResponse) {
	t.Helper()
	if response.action != "" || response.Code != code.ErrorInvalidParams.Code() || response.Status {
		t.Fatalf("unexpected malformed-sync response: action=%q code=%d status=%v payload=%s", response.action, response.Code, response.Status, response.payload)
	}
	var envelope map[string]json.RawMessage
	if err := json.Unmarshal(response.payload, &envelope); err != nil {
		t.Fatalf("decode malformed-sync v1 envelope: %v", err)
	}
	wantKeys := []string{"code", "status", "message", "data", "details"}
	if len(envelope) != len(wantKeys) {
		t.Fatalf("malformed sync did not use the exact v1 error envelope: keys=%v payload=%s", envelope, response.payload)
	}
	for _, key := range wantKeys {
		if _, ok := envelope[key]; !ok {
			t.Fatalf("malformed sync v1 envelope missing %q: payload=%s", key, response.payload)
		}
	}
}

func TestV1BusinessActionBeforeAuthorizationUsesMessageInterceptor(t *testing.T) {
	testApp := internalapp.NewTestApp(&internalapp.Services{})
	h := newV1RouterHarness(t, testApp, false)

	h.send(`NoteSync|{"vault":"main"}`)
	response := h.receive()
	if response.action != "" || response.Code != code.ErrorNotUserAuthToken.Code() || response.Status {
		t.Fatalf("unexpected pre-auth v1 response from message interceptor: action=%q code=%d status=%v payload=%s", response.action, response.Code, response.Status, response.payload)
	}
}

func TestV1NoteSyncSendsEndBeforeQueuedModify(t *testing.T) {
	noteSvc := new(svcmocks.MockNoteService)
	vaultSvc := new(svcmocks.MockVaultService)
	testApp := internalapp.NewTestApp(&internalapp.Services{NoteService: noteSvc, VaultService: vaultSvc})
	vaultSvc.On("GetOrCreate", mock.Anything, int64(7), "main").
		Return(&domain.Vault{ID: 1, UID: 7, Name: "main"}, nil).Once()
	noteSvc.On("ListByLastTime", mock.Anything, int64(7), mock.MatchedBy(func(request *dto.NoteSyncRequest) bool {
		return request.Vault == "main" && request.Context == "note-context" && request.LastTime == 0
	})).Return([]*dto.NoteDTO{{
		Path:             "notes/a.md",
		PathHash:         "note-hash",
		Content:          "alpha|beta",
		ContentHash:      "note-content-hash",
		Ctime:            10,
		Mtime:            20,
		UpdatedTimestamp: 30,
	}}, nil).Once()

	h := newV1RouterHarness(t, testApp, true)
	h.send(`NoteSync|{"context":"note-context","vault":"main","lastTime":0,"notes":[],"delNotes":[],"missingNotes":[]}`)

	assertV1SyncEnd(t, h.receive(), NoteSyncEnd, "note-context", 0, 1, 0, 0)
	modify := h.receive()
	if modify.action != NoteSyncModify || modify.Code != code.Success.Code() || !modify.Status || modify.Vault != "main" || modify.Context != "note-context" {
		t.Fatalf("unexpected queued NoteSyncModify envelope: action=%q code=%d status=%v payload=%s", modify.action, modify.Code, modify.Status, modify.payload)
	}
	var data dto.NoteSyncModifyMessage
	if err := json.Unmarshal(modify.Data, &data); err != nil {
		t.Fatalf("decode NoteSyncModify data: %v", err)
	}
	if data.Path != "notes/a.md" || data.PathHash != "note-hash" || data.Content != "alpha|beta" || data.ContentHash != "note-content-hash" || data.Ctime != 10 || data.Mtime != 20 || data.UpdatedTimestamp != 30 {
		t.Fatalf("unexpected queued NoteSyncModify data: %+v", data)
	}
	noteSvc.AssertExpectations(t)
	vaultSvc.AssertExpectations(t)
}

func TestV1FileSyncSendsEndBeforeQueuedDelete(t *testing.T) {
	fileSvc := new(svcmocks.MockFileService)
	vaultSvc := new(svcmocks.MockVaultService)
	testApp := internalapp.NewTestApp(&internalapp.Services{FileService: fileSvc, VaultService: vaultSvc})
	vaultSvc.On("GetOrCreate", mock.Anything, int64(7), "main").
		Return(&domain.Vault{ID: 1, UID: 7, Name: "main"}, nil).Once()
	fileSvc.On("ListByLastTime", mock.Anything, int64(7), mock.MatchedBy(func(request *dto.FileSyncRequest) bool {
		return request.Vault == "main" && request.Context == "file-context" && request.LastTime == 0 && len(request.Files) == 1 && request.Files[0].PathHash == "file-hash"
	})).Return([]*dto.FileDTO{{
		Action:           "delete",
		Path:             "files/old.bin",
		PathHash:         "file-hash",
		ContentHash:      "file-content-hash",
		Size:             99,
		Ctime:            40,
		Mtime:            50,
		UpdatedTimestamp: 60,
	}}, nil).Once()

	h := newV1RouterHarness(t, testApp, true)
	h.send(`FileSync|{"context":"file-context","vault":"main","lastTime":0,"files":[{"path":"files/old.bin","pathHash":"file-hash","contentHash":"client-hash","mtime":1}],"delFiles":[],"missingFiles":[]}`)

	assertV1SyncEnd(t, h.receive(), FileSyncEnd, "file-context", 0, 0, 0, 1)
	deleted := h.receive()
	if deleted.action != FileSyncDelete || deleted.Code != code.Success.Code() || !deleted.Status || deleted.Vault != "main" || deleted.Context != "file-context" {
		t.Fatalf("unexpected queued FileSyncDelete envelope: action=%q code=%d status=%v payload=%s", deleted.action, deleted.Code, deleted.Status, deleted.payload)
	}
	var data dto.FileSyncDeleteMessage
	if err := json.Unmarshal(deleted.Data, &data); err != nil {
		t.Fatalf("decode FileSyncDelete data: %v", err)
	}
	if data.Path != "files/old.bin" || data.PathHash != "file-hash" || data.Size != 99 || data.Ctime != 40 || data.Mtime != 50 || data.UpdatedTimestamp != 60 {
		t.Fatalf("unexpected queued FileSyncDelete data: %+v", data)
	}
	fileSvc.AssertExpectations(t)
	vaultSvc.AssertExpectations(t)
}

func TestV1SyncInvalidJSONKeepsErrorEnvelope(t *testing.T) {
	testApp := internalapp.NewTestApp(&internalapp.Services{})
	h := newV1RouterHarness(t, testApp, true)

	h.send("NoteSync|{")
	assertV1MalformedSyncError(t, h.receive())

	h.send("FileSync|{")
	assertV1MalformedSyncError(t, h.receive())
}
