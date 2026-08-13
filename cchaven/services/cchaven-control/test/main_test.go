// Package test 存放需要真实 PostgreSQL 的集成与端到端测试。
//
// 不带 DB 依赖的单元测试与被测包放在一起；这里只关心跨层链路。
package test

import (
	"fmt"
	"os"
	"testing"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/testsupport"
)

func TestMain(m *testing.M) {
	if _, err := testsupport.StartPostgres(); err != nil {
		fmt.Fprintf(os.Stderr, "无法启动测试数据库: %v\n", err)
		os.Exit(1)
	}

	code := m.Run()
	testsupport.StopPostgres()
	os.Exit(code)
}
