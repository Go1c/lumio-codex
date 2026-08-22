package errors

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/gin-gonic/gin"
	"github.com/haierkeys/fast-note-sync-service/pkg/code"
)

func TestErrorResponseKeepsHTTP200AndStatusFalse(t *testing.T) {
	gin.SetMode(gin.TestMode)
	w := httptest.NewRecorder()
	c, _ := gin.CreateTestContext(w)
	c.Request = httptest.NewRequest(http.MethodPost, "/api/user/login", nil)

	ErrorResponse(c, code.ErrorUserLoginPasswordFailed)

	if w.Code != http.StatusOK {
		t.Fatalf("business errors must stay HTTP 200, got %d", w.Code)
	}
	var body map[string]any
	if err := json.Unmarshal(w.Body.Bytes(), &body); err != nil {
		t.Fatalf("decode: %v body=%s", err, w.Body.String())
	}
	if body["code"] != float64(code.ErrorUserLoginPasswordFailed.Code()) {
		t.Fatalf("code=%v want %d", body["code"], code.ErrorUserLoginPasswordFailed.Code())
	}
	status, ok := body["status"].(bool)
	if !ok || status {
		t.Fatalf("envelope must include status=false so clients can tell failure without HTTP 4xx; body=%s", w.Body.String())
	}
}
