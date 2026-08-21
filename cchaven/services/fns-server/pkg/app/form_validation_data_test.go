package app

import (
	"encoding/json"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/gin-gonic/gin"
	"github.com/haierkeys/fast-note-sync-service/pkg/code"
)

func TestValidErrorsResponseDataIsJSONArrayNotString(t *testing.T) {
	gin.SetMode(gin.TestMode)
	w := httptest.NewRecorder()
	c, _ := gin.CreateTestContext(w)
	c.Request = httptest.NewRequest("POST", "/api/user/login", nil)

	errs := ValidErrors{&ValidError{Key: "Password", Message: "required"}}
	NewResponse(c).ToResponse(code.ErrorInvalidParams.WithData(errs.ResponseData()))

	var parsed map[string]any
	if err := json.Unmarshal(w.Body.Bytes(), &parsed); err != nil {
		t.Fatalf("response is not JSON: %v body=%s", err, w.Body.String())
	}
	data, ok := parsed["data"]
	if !ok {
		t.Fatalf("missing data field: %s", w.Body.String())
	}
	if _, isString := data.(string); isString {
		t.Fatalf("data is a JSON string (double-encoded): %#v", data)
	}
	arr, ok := data.([]any)
	if !ok {
		t.Fatalf("data want JSON array, got %T (%#v)", data, data)
	}
	if len(arr) != 1 {
		t.Fatalf("data length = %d, want 1", len(arr))
	}
}

func TestBindAndValidIllegalJSONReturnsParseableError(t *testing.T) {
	gin.SetMode(gin.TestMode)
	w := httptest.NewRecorder()
	c, _ := gin.CreateTestContext(w)
	c.Request = httptest.NewRequest("POST", "/api/user/login", strings.NewReader("not-json"))
	c.Request.Header.Set("Content-Type", "application/json")

	var obj struct {
		Password string `json:"password" binding:"required"`
	}
	ok, errs := BindAndValid(c, &obj)
	if ok {
		t.Fatal("illegal JSON should fail bind")
	}
	if len(errs) == 0 || errs.ErrorsToString() == "" {
		t.Fatalf("illegal JSON left errs empty (data would be string null): %#v", errs)
	}
}
