package routers

import (
	"encoding/json"
	"io/fs"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"testing/fstest"

	"github.com/gin-gonic/gin"
)

func openAPITestFS() fs.FS {
	return fstest.MapFS{
		"docs/swagger.yaml": {
			Data: []byte("basePath: /\ndefinitions:\n  api_router.HealthResponse:\n    type: object\n"),
		},
		"docs/swagger.json": {
			Data: []byte(`{"swagger":"2.0","info":{"title":"Fast Note Sync Service HTTP API"},"basePath":"/"}`),
		},
		"docs/test_ws_debug.html": {Data: []byte("<html></html>")},
	}
}

func TestOpenAPIJSONServesJSONNotYAML(t *testing.T) {
	gin.SetMode(gin.TestMode)
	r := gin.New()
	registerOpenAPIRoutes(r, openAPITestFS())

	w := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/openapi.json", nil)
	r.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, want 200, body=%s", w.Code, w.Body.String())
	}
	ct := w.Header().Get("Content-Type")
	if !strings.Contains(ct, "application/json") {
		t.Fatalf("Content-Type = %q, want application/json", ct)
	}
	body := w.Body.Bytes()
	if !json.Valid(body) {
		t.Fatalf("GET /openapi.json body is not JSON (looks like YAML?): %q", truncateForTest(body, 120))
	}
	var doc map[string]any
	if err := json.Unmarshal(body, &doc); err != nil {
		t.Fatalf("Unmarshal JSON: %v", err)
	}
	if _, ok := doc["swagger"]; !ok {
		t.Fatalf("JSON OpenAPI doc missing swagger field: %#v", doc)
	}
}

func TestOpenAPIYAMLStillServesYAML(t *testing.T) {
	gin.SetMode(gin.TestMode)
	r := gin.New()
	registerOpenAPIRoutes(r, openAPITestFS())

	w := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/openapi.yaml", nil)
	r.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, want 200, body=%s", w.Code, w.Body.String())
	}
	body := strings.TrimSpace(w.Body.String())
	if !strings.HasPrefix(body, "basePath:") {
		t.Fatalf("GET /openapi.yaml body is not YAML: %q", truncateForTest([]byte(body), 120))
	}
	if json.Valid([]byte(body)) {
		t.Fatalf("GET /openapi.yaml unexpectedly valid JSON")
	}
}

func truncateForTest(b []byte, n int) string {
	if len(b) <= n {
		return string(b)
	}
	return string(b[:n]) + "..."
}
