// Package db 提供 PostgreSQL 连接池与随代码发布的 SQL 迁移执行器。
package db

import (
	"context"
	"fmt"
	"io/fs"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/Go1c/fns-workspace/services/cchaven-control/migrations"
)

// Pool 是服务使用的连接池类型别名，便于在仓储层书写。
type Pool = pgxpool.Pool

// Connect 建立连接池并确认数据库可达。
func Connect(ctx context.Context, databaseURL string) (*Pool, error) {
	cfg, err := pgxpool.ParseConfig(databaseURL)
	if err != nil {
		return nil, fmt.Errorf("db: 解析连接串失败: %w", err)
	}
	cfg.MaxConns = 16
	cfg.MaxConnLifetime = time.Hour

	// 固定会话时区为 UTC。DAU、留存与订单号都依赖 ::date 转换，
	// 让它跟随服务器本地时区会导致同一时刻在不同部署环境落到不同的「天」。
	if cfg.ConnConfig.RuntimeParams == nil {
		cfg.ConnConfig.RuntimeParams = map[string]string{}
	}
	cfg.ConnConfig.RuntimeParams["timezone"] = "UTC"

	pool, err := pgxpool.NewWithConfig(ctx, cfg)
	if err != nil {
		return nil, fmt.Errorf("db: 建立连接池失败: %w", err)
	}

	pingCtx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	if err := pool.Ping(pingCtx); err != nil {
		pool.Close()
		return nil, fmt.Errorf("db: 数据库不可达: %w", err)
	}
	return pool, nil
}

// Migrate 按文件名顺序执行尚未应用的迁移脚本。每个脚本在独立事务中执行，
// 失败即整体回滚，保证不会留下半套表结构。
func Migrate(ctx context.Context, pool *Pool) error {
	if _, err := pool.Exec(ctx, `
		CREATE TABLE IF NOT EXISTS schema_migrations (
			version     bigint      PRIMARY KEY,
			name        text        NOT NULL,
			applied_at  timestamptz NOT NULL DEFAULT now()
		)`); err != nil {
		return fmt.Errorf("db: 创建 schema_migrations 失败: %w", err)
	}

	applied := map[int64]bool{}
	rows, err := pool.Query(ctx, `SELECT version FROM schema_migrations`)
	if err != nil {
		return fmt.Errorf("db: 读取已应用迁移失败: %w", err)
	}
	for rows.Next() {
		var v int64
		if err := rows.Scan(&v); err != nil {
			rows.Close()
			return err
		}
		applied[v] = true
	}
	rows.Close()
	if err := rows.Err(); err != nil {
		return err
	}

	files, err := loadMigrations()
	if err != nil {
		return err
	}

	for _, m := range files {
		if applied[m.version] {
			continue
		}
		if err := applyOne(ctx, pool, m); err != nil {
			return err
		}
	}
	return nil
}

type migration struct {
	version int64
	name    string
	body    string
}

func loadMigrations() ([]migration, error) {
	entries, err := fs.ReadDir(migrations.FS, ".")
	if err != nil {
		return nil, fmt.Errorf("db: 读取迁移目录失败: %w", err)
	}

	var out []migration
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".sql") {
			continue
		}
		prefix, _, ok := strings.Cut(e.Name(), "_")
		if !ok {
			return nil, fmt.Errorf("db: 迁移文件名缺少版本前缀: %s", e.Name())
		}
		version, err := strconv.ParseInt(prefix, 10, 64)
		if err != nil {
			return nil, fmt.Errorf("db: 迁移文件名版本前缀非法: %s", e.Name())
		}
		body, err := migrations.FS.ReadFile(e.Name())
		if err != nil {
			return nil, err
		}
		out = append(out, migration{version: version, name: e.Name(), body: string(body)})
	}

	sort.Slice(out, func(i, j int) bool { return out[i].version < out[j].version })
	return out, nil
}

func applyOne(ctx context.Context, pool *Pool, m migration) error {
	tx, err := pool.Begin(ctx)
	if err != nil {
		return fmt.Errorf("db: 开启迁移事务失败 (%s): %w", m.name, err)
	}
	defer func() { _ = tx.Rollback(ctx) }()

	if _, err := tx.Exec(ctx, m.body); err != nil {
		return fmt.Errorf("db: 执行迁移失败 (%s): %w", m.name, err)
	}
	if _, err := tx.Exec(ctx,
		`INSERT INTO schema_migrations (version, name) VALUES ($1, $2)`, m.version, m.name); err != nil {
		return fmt.Errorf("db: 记录迁移版本失败 (%s): %w", m.name, err)
	}
	return tx.Commit(ctx)
}

// InTx 在一个事务中执行 fn；fn 返回错误或 panic 时回滚。
// 试用发放、邀请奖励等跨表写入必须走这里，保证原子性。
func InTx(ctx context.Context, pool *Pool, fn func(pgx.Tx) error) error {
	tx, err := pool.Begin(ctx)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback(ctx) }()

	if err := fn(tx); err != nil {
		return err
	}
	return tx.Commit(ctx)
}
