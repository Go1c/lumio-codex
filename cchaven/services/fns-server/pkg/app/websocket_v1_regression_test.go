package app

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http/httptest"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/gin-gonic/gin"
	"github.com/haierkeys/fast-note-sync-service/pkg/code"
	"github.com/lxzan/gws"
	"go.uber.org/zap"
	"go.uber.org/zap/zaptest/observer"
)

const v1TestTokenKey = "v1-regression-secret"

type v1TestApp struct{}

func (v1TestApp) Logger() *zap.Logger { return zap.NewNop() }

func (v1TestApp) SubmitTask(ctx context.Context, task func(context.Context) error) error {
	return task(ctx)
}

func (v1TestApp) SubmitTaskAsync(ctx context.Context, task func(context.Context) error) error {
	return task(ctx)
}

func (v1TestApp) Version() VersionInfo { return VersionInfo{Version: "v1-test"} }

func (v1TestApp) CheckVersion(string) CheckVersionInfo { return CheckVersionInfo{} }

func (v1TestApp) Validator() ValidatorInterface { return nil }

func (v1TestApp) IsReturnSuccess() bool { return true }

func (v1TestApp) GetAuthTokenKey() string { return v1TestTokenKey }

func (v1TestApp) IsProductionMode() bool { return true }

func (v1TestApp) GetTokenService() any { return nil }

type v1ClientEvents struct {
	gws.BuiltinEventHandler
	messages chan []byte
	closed   chan error
}

func (e *v1ClientEvents) OnMessage(_ *gws.Conn, message *gws.Message) {
	defer message.Close()
	payload := append([]byte(nil), message.Data.Bytes()...)
	select {
	case e.messages <- payload:
	default:
	}
}

func (e *v1ClientEvents) OnClose(_ *gws.Conn, err error) {
	select {
	case e.closed <- err:
	default:
	}
}

type v1WireResponse struct {
	action  string
	payload []byte
	Code    int             `json:"code"`
	Status  bool            `json:"status"`
	Message any             `json:"message"`
	Data    json.RawMessage `json:"data"`
	Details any             `json:"details"`
	Vault   any             `json:"vault"`
	Context any             `json:"context"`
}

type v1WebsocketHarness struct {
	t      *testing.T
	wss    *WebsocketServer
	server *httptest.Server
	conn   *gws.Conn
	events *v1ClientEvents
}

func newV1WebsocketHarness(t *testing.T, configure func(*WebsocketServer)) *v1WebsocketHarness {
	t.Helper()
	gin.SetMode(gin.TestMode)

	wss := NewWebsocketServer(WSConfig{}, v1TestApp{})
	wss.UseUserVerify(func(_ *WebsocketClient, uid int64) (*UserSelectEntity, error) {
		return &UserSelectEntity{UID: uid, Nickname: "v1-user"}, nil
	})
	if configure != nil {
		configure(wss)
	}

	router := gin.New()
	router.GET("/api/user/sync", wss.Run())
	server := httptest.NewServer(router)
	events := &v1ClientEvents{
		messages: make(chan []byte, 8),
		closed:   make(chan error, 1),
	}
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

	h := &v1WebsocketHarness{t: t, wss: wss, server: server, conn: conn, events: events}
	go conn.ReadLoop()
	t.Cleanup(h.close)
	return h
}

func (h *v1WebsocketHarness) close() {
	_ = h.conn.WriteClose(1000, []byte("test complete"))
	_ = h.conn.NetConn().Close()
	h.wss.CloseAllConnections()
	h.server.Close()
	h.wss.WaitAllClosed(time.Second)
}

func (h *v1WebsocketHarness) send(payload string) {
	h.t.Helper()
	if err := h.conn.WriteString(payload); err != nil {
		h.t.Fatalf("send websocket message: %v", err)
	}
}

func (h *v1WebsocketHarness) sendBinary(payload []byte) {
	h.t.Helper()
	if err := h.conn.WriteMessage(gws.OpcodeBinary, payload); err != nil {
		h.t.Fatalf("send binary websocket message: %v", err)
	}
}

func (h *v1WebsocketHarness) receive() v1WireResponse {
	h.t.Helper()
	select {
	case raw := <-h.events.messages:
		response := v1WireResponse{payload: raw}
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
		return v1WireResponse{}
	}
}

func (h *v1WebsocketHarness) authorize() v1WireResponse {
	h.t.Helper()
	token, err := NewTokenManager(TokenConfig{SecretKey: v1TestTokenKey}).Generate(7, "v1-user", "", 11, "nonce")
	if err != nil {
		h.t.Fatalf("generate JWT: %v", err)
	}
	h.send("Authorization|" + token)
	return h.receive()
}

func requireV1AuthorizationSuccess(t *testing.T, response v1WireResponse) {
	t.Helper()
	if response.action != "Authorization" || response.Code != code.Success.Code() || !response.Status {
		t.Fatalf("unexpected authorization response: action=%q code=%d status=%v payload=%s", response.action, response.Code, response.Status, response.payload)
	}
}

func TestWebSocketTokenScopeUpdateDoesNotLogRawScope(t *testing.T) {
	core, observed := observer.New(zap.InfoLevel)
	SetWSLogger(zap.New(core))
	t.Cleanup(func() { SetWSLogger(zap.NewNop()) })

	client := &WebsocketClient{TokenID: 11}
	clients := make(ConnStorage)
	clients[nil] = client
	server := &WebsocketServer{userClients: map[string]ConnStorage{"7": clients}}
	sensitiveScope := "p:ws c:fns-agent f:workspace_rw scope-log-marker"

	server.UpdateTokenScope(7, 11, sensitiveScope)

	if client.Scope != sensitiveScope {
		t.Fatalf("client scope = %q, want update to be applied", client.Scope)
	}
	entries := observed.FilterMessage("WS UpdateTokenScope").AllUntimed()
	if len(entries) != 1 {
		t.Fatalf("scope update log count = %d, want 1", len(entries))
	}
	for key, value := range entries[0].ContextMap() {
		valueString, _ := value.(string)
		if strings.Contains(strings.ToLower(key), "scope") || strings.Contains(strings.ToLower(valueString), "scope-log-marker") {
			t.Fatalf("scope update log exposed authorization data: %s=%v", key, value)
		}
	}
}

func TestWebSocketModuleConfigurationIsRaceSafe(t *testing.T) {
	const iterations = 1_000
	var workers sync.WaitGroup
	workers.Add(4)

	go func() {
		defer workers.Done()
		for range iterations {
			SetWSLogger(zap.NewNop())
		}
	}()
	go func() {
		defer workers.Done()
		for range iterations {
			log(LogInfo, "race probe")
		}
	}()
	go func() {
		defer workers.Done()
		for i := range iterations {
			SetWSProductionMode(i%2 == 0)
		}
	}()
	go func() {
		defer workers.Done()
		for range iterations {
			_ = isDevelopmentMode()
		}
	}()

	workers.Wait()
}

func TestV1TextFrameSplitsOnFirstPipe(t *testing.T) {
	h := newV1WebsocketHarness(t, func(wss *WebsocketServer) {
		wss.Use("Echo", func(c *WebsocketClient, msg *WebSocketMessage) {
			var request struct {
				Value string `json:"value"`
			}
			if err := json.Unmarshal(msg.Data, &request); err != nil {
				t.Errorf("handler received invalid JSON: %v", err)
				return
			}
			c.ToResponse(code.Success.WithData(request), "EchoAck")
		})
	})
	requireV1AuthorizationSuccess(t, h.authorize())

	h.send(`Echo|{"value":"left|right"}`)
	response := h.receive()
	if response.action != "EchoAck" || response.Code != code.Success.Code() || !response.Status {
		t.Fatalf("unexpected v1 echo envelope: action=%q code=%d status=%v", response.action, response.Code, response.Status)
	}
	var data struct {
		Value string `json:"value"`
	}
	if err := json.Unmarshal(response.Data, &data); err != nil {
		t.Fatalf("decode echo data: %v", err)
	}
	if data.Value != "left|right" {
		t.Fatalf("embedded pipe was not preserved: got %q", data.Value)
	}
}

func TestV1BusinessActionBeforeAuthorizationIsRejected(t *testing.T) {
	var invoked atomic.Bool
	h := newV1WebsocketHarness(t, func(wss *WebsocketServer) {
		wss.Use("BusinessAction", func(_ *WebsocketClient, _ *WebSocketMessage) {
			invoked.Store(true)
		})
		wss.UseInterceptor(func(c *WebsocketClient, _ *WebSocketMessage) bool {
			if c.User != nil {
				return true
			}
			c.ToResponse(code.ErrorNotUserAuthToken)
			return false
		})
	})

	h.send(`BusinessAction|{"value":1}`)
	response := h.receive()
	if response.action != "" || response.Code != code.ErrorNotUserAuthToken.Code() || response.Status {
		t.Fatalf("unexpected pre-auth v1 response: action=%q code=%d status=%v payload=%s", response.action, response.Code, response.Status, response.payload)
	}
	if invoked.Load() {
		t.Fatal("business handler ran before authorization")
	}
}

func TestV1AuthorizationAcceptsValidJWT(t *testing.T) {
	var tokenVerified atomic.Bool
	var userVerified atomic.Bool
	h := newV1WebsocketHarness(t, func(wss *WebsocketServer) {
		wss.UseTokenVerify(func(_ context.Context, uid, tokenID int64, nonce, _, _, _, _, _ string) (string, string, error) {
			if uid != 7 || tokenID != 11 || nonce != "nonce" {
				t.Errorf("unexpected token claims: uid=%d tokenID=%d nonce=%q", uid, tokenID, nonce)
			}
			tokenVerified.Store(true)
			return "*", "", nil
		})
		wss.UseUserVerify(func(_ *WebsocketClient, uid int64) (*UserSelectEntity, error) {
			if uid != 7 {
				t.Errorf("unexpected user verification uid: %d", uid)
			}
			userVerified.Store(true)
			return &UserSelectEntity{UID: uid, Nickname: "v1-user"}, nil
		})
	})

	response := h.authorize()
	requireV1AuthorizationSuccess(t, response)
	if !tokenVerified.Load() || !userVerified.Load() {
		t.Fatalf("authorization verification hooks not called: token=%v user=%v", tokenVerified.Load(), userVerified.Load())
	}
	var data map[string]string
	if err := json.Unmarshal(response.Data, &data); err != nil {
		t.Fatalf("decode authorization version data: %v", err)
	}
	if data["version"] != "v1-test" {
		t.Fatalf("authorization version = %q, want v1-test", data["version"])
	}
}

func TestV1AuthorizationRejectsMalformedJWT(t *testing.T) {
	h := newV1WebsocketHarness(t, nil)

	h.send("Authorization|not-a-jwt")
	response := h.receive()
	if response.action != "Authorization" || response.Code != code.ErrorInvalidUserAuthToken.Code() || response.Status {
		t.Fatalf("unexpected invalid-auth response: action=%q code=%d status=%v payload=%s", response.action, response.Code, response.Status, response.payload)
	}

	select {
	case <-h.events.closed:
	case <-time.After(5 * time.Second):
		t.Fatal("timed out waiting for malformed-JWT connection close")
	}
}

func TestV1Binary00DispatchesPayloadWithoutPrefix(t *testing.T) {
	want := []byte{0x00, 0x01, '|', 0xff, 0x7f}
	received := make(chan []byte, 1)
	var textHandlerCalls atomic.Int32
	var protobufHandlerCalls atomic.Int32
	h := newV1WebsocketHarness(t, func(wss *WebsocketServer) {
		wss.UseBinary("00", func(_ *WebsocketClient, payload []byte) {
			received <- append([]byte(nil), payload...)
		})
		wss.Use("00", func(_ *WebsocketClient, _ *WebSocketMessage) {
			textHandlerCalls.Add(1)
		})
		wss.EnvelopeDecoder = func([]byte) (string, []byte, error) {
			protobufHandlerCalls.Add(1)
			return "00", nil, nil
		}
	})
	requireV1AuthorizationSuccess(t, h.authorize())

	frame := append([]byte("00"), want...)
	h.sendBinary(frame)
	select {
	case got := <-received:
		if !bytes.Equal(got, want) {
			t.Fatalf("binary handler payload = %v, want %v", got, want)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("timed out waiting for binary 00 dispatch")
	}
	if textHandlerCalls.Load() != 0 || protobufHandlerCalls.Load() != 0 {
		t.Fatalf("binary 00 reached another dispatch path: text=%d protobuf=%d", textHandlerCalls.Load(), protobufHandlerCalls.Load())
	}
}
