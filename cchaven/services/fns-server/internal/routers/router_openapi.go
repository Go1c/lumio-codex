package routers

import (
	"io/fs"
	"net/http"

	"github.com/gin-gonic/gin"
	_ "github.com/haierkeys/fast-note-sync-service/docs"
	swaggerFiles "github.com/swaggo/files"
	ginSwagger "github.com/swaggo/gin-swagger"
)

func registerOpenAPIRoutes(r *gin.Engine, frontendFiles fs.FS) {
	// Swagger UI (outside auth group to ensure public access)
	// Swagger UI (放在 auth 组外，确保可以公开访问)
	r.GET("/docs/*any", func(c *gin.Context) {
		p := c.Param("any")
		if p == "" || p == "/" {
			c.Redirect(http.StatusMovedPermanently, "/docs/index.html")
			return
		}
		ginSwagger.WrapHandler(swaggerFiles.Handler)(c)
	})

	// Read debug page from embedded FS
	debugPageContent, _ := fs.ReadFile(frontendFiles, "docs/test_ws_debug.html")
	r.GET("/ws_debug", func(c *gin.Context) {
		c.Data(http.StatusOK, "text/html; charset=utf-8", debugPageContent)
	})

	// Read swagger files from embedded FS. /openapi.json must be real JSON;
	// the YAML original is served at /openapi.yaml.
	swaggerJSON, _ := fs.ReadFile(frontendFiles, "docs/swagger.json")
	r.GET("/openapi/", func(c *gin.Context) {
		c.Redirect(http.StatusMovedPermanently, "/openapi.json")
	})
	r.GET("/openapi.json", func(c *gin.Context) {
		c.Data(http.StatusOK, "application/json; charset=utf-8", swaggerJSON)
	})
	swaggerYAML, _ := fs.ReadFile(frontendFiles, "docs/swagger.yaml")
	r.GET("/openapi.yaml", func(c *gin.Context) {
		c.Data(http.StatusOK, "application/x-yaml; charset=utf-8", swaggerYAML)
	})
}
