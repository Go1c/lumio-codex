// Package migrations 以 embed 方式携带随代码发布的 SQL 迁移脚本。
package migrations

import "embed"

// FS 包含全部迁移脚本，文件名格式为 {版本号}_{说明}.sql，按版本号升序执行。
//
//go:embed *.sql
var FS embed.FS
