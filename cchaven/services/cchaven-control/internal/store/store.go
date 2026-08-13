// Package store 是数据访问层：手写 SQL + pgx，不使用 ORM 与代码生成。
//
// 所有函数的第一个参数都是 Querier，*pgxpool.Pool 与 pgx.Tx 都满足它，
// 因此同一份仓储代码既能独立调用，也能被组合进事务（如试用发放 + 邀请奖励）。
package store

import (
	"context"
	"errors"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
)

// ErrNotFound 表示查询未命中。调用方负责翻译为对应的 API 错误。
var ErrNotFound = errors.New("store: 记录不存在")

// Querier 是 *pgxpool.Pool 与 pgx.Tx 的公共子集。
type Querier interface {
	Exec(ctx context.Context, sql string, args ...any) (pgconn.CommandTag, error)
	Query(ctx context.Context, sql string, args ...any) (pgx.Rows, error)
	QueryRow(ctx context.Context, sql string, args ...any) pgx.Row
}

// normalizeErr 把 pgx 的「无行」错误翻译为本包的 ErrNotFound。
func normalizeErr(err error) error {
	if errors.Is(err, pgx.ErrNoRows) {
		return ErrNotFound
	}
	return err
}

// IsUniqueViolation 报告错误是否为唯一约束冲突。
//
// 试用发放与邀请奖励依赖数据库唯一索引做并发兜底，命中冲突属于预期路径而非故障。
func IsUniqueViolation(err error) bool {
	var pgErr *pgconn.PgError
	return errors.As(err, &pgErr) && pgErr.Code == "23505"
}
