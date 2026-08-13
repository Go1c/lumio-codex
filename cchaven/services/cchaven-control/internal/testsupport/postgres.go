// Package testsupport 提供集成测试所需的真实 PostgreSQL 与 HTTP 测试夹具。
//
// 默认用 embedded-postgres 拉起一个真实的 PostgreSQL（首次运行会下载二进制并缓存到
// .embedded-postgres/），因此 `go test ./...` 无需 Docker 也无需本机安装 PostgreSQL。
// 若设置了 CCHAVEN_TEST_DATABASE_URL，则直接复用该实例。
package testsupport

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"sync"
	"time"

	embeddedpostgres "github.com/fergusstrange/embedded-postgres"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/db"
)

const (
	embeddedPort     = 15432
	embeddedUser     = "cchaven"
	embeddedPassword = "cchaven"
	embeddedDatabase = "cchaven_test"
)

var (
	instance    *embeddedpostgres.EmbeddedPostgres
	databaseURL string
	startOnce   sync.Once
	startErr    error
)

// StartPostgres 启动测试数据库并返回连接串。多次调用只启动一次。
// 调用方应在 TestMain 中配对调用 StopPostgres。
func StartPostgres() (string, error) {
	startOnce.Do(func() {
		if external := os.Getenv("CCHAVEN_TEST_DATABASE_URL"); external != "" {
			databaseURL = external
			return
		}

		cache := filepath.Join(repoRoot(), ".embedded-postgres")
		if err := os.MkdirAll(cache, 0o755); err != nil {
			startErr = err
			return
		}

		instance = embeddedpostgres.NewDatabase(embeddedpostgres.DefaultConfig().
			Username(embeddedUser).
			Password(embeddedPassword).
			Database(embeddedDatabase).
			Port(embeddedPort).
			CachePath(cache).
			RuntimePath(filepath.Join(cache, "runtime")).
			DataPath(filepath.Join(cache, "data")).
			BinariesPath(filepath.Join(cache, "bin")).
			StartTimeout(90 * time.Second))

		if err := instance.Start(); err != nil {
			startErr = fmt.Errorf("testsupport: 启动 embedded postgres 失败: %w", err)
			instance = nil
			return
		}

		databaseURL = fmt.Sprintf("postgres://%s:%s@127.0.0.1:%d/%s?sslmode=disable",
			embeddedUser, embeddedPassword, embeddedPort, embeddedDatabase)
	})

	return databaseURL, startErr
}

// StopPostgres 关闭由本包启动的数据库；使用外部实例时为空操作。
func StopPostgres() {
	if instance != nil {
		_ = instance.Stop()
		instance = nil
	}
}

// MigrateOnce 在测试数据库上执行迁移。
func MigrateOnce(ctx context.Context, pool *db.Pool) error {
	return db.Migrate(ctx, pool)
}

// repoRoot 定位服务模块根目录，使缓存路径不受测试工作目录影响。
func repoRoot() string {
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		return "."
	}
	// file = <root>/internal/testsupport/postgres.go
	return filepath.Dir(filepath.Dir(filepath.Dir(file)))
}
