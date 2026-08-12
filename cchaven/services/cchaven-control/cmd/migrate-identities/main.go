// Command migrate-identities 把 cchaven-control 的存量终端用户对齐到 Lumio 账号中心（Sub2API）。
//
// 身份收口之后，users 表退化为「CC 业务侧的影子账号」，谁是谁由 sub2api_identities
// 里的映射决定。迁移前建的账号还没有这条映射，本工具负责补上：按邮箱在 Sub2API
// 找到（或建立）对应用户，然后写映射。
//
// # 执行前置条件（缺一不可）
//
//  1. 控制面已升到含 0003_sub2api_identities.sql 的版本，表结构就位；
//  2. 已对生产库做过一次可恢复的备份，并确认恢复演练通过；
//  3. 手上有 Sub2API 的管理员令牌，且已与账号中心确认下面两个端点的契约
//     （见「Sub2API 契约」一节）——契约对不上时先改这里，不要改数据；
//  4. 先在预发库上以默认的 dry-run 跑一遍，人工核对报告；
//  5. 真正写入需要显式加 -apply，并且**必须事先取得负责人确认**：
//     这一步会在账号中心创建真实用户。
//
// # 用法
//
//	export CCHAVEN_DATABASE_URL='postgres://…'
//	export CCHAVEN_SUB2API_BASE='https://api.lumio.games'
//	export CCHAVEN_SUB2API_ADMIN_TOKEN='…'      # 只从环境读，不进 argv、不打日志
//
//	go run ./cmd/migrate-identities                # dry-run：只查、只报告，不写任何一侧
//	go run ./cmd/migrate-identities -apply         # 真正写入（需负责人确认）
//	go run ./cmd/migrate-identities -only alice@example.com   # 先拿一个账号试水
//
// # 幂等性
//
// 已经有映射的用户直接跳过，因此中断后重跑安全。Sub2API 侧的建号走「先查后建，
// 撞 409 再查一次」，并发或重跑都不会产生重复账号。
//
// # 口令怎么办
//
// 本工具**不迁移口令**，也不生成口令：本地存的是 argon2 摘要，不可逆；生成一个
// 临时口令又意味着要把它安全地送到用户手里。迁移出来的账号一律没有可用口令，
// 用户首次登录走账号中心的「忘记密码」重设。这一点必须提前在公告里讲清楚。
//
// # Sub2API 契约（与账号中心对齐后再执行）
//
//	查用户：GET  {base}{lookup}?email=<email>   →  200 {"data":{"id":…,"email":…}} / 404
//	建用户：POST {base}{create}  {"email":…,"email_verified":true,"source":"cchaven-migration"}
//	                            →  201 {"data":{"id":…}}；邮箱已存在返回 409
//
// 路径可用 CCHAVEN_SUB2API_ADMIN_LOOKUP_PATH / _CREATE_PATH 覆盖，省得为了改一段
// 路径去动代码。响应信封同时兼容 {"data":{…}} 与裸对象。
package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
)

const (
	defaultLookupPath = "/api/v1/admin/users"
	defaultCreatePath = "/api/v1/admin/users"
	requestTimeout    = 15 * time.Second
)

type options struct {
	apply     bool
	onlyEmail string
	limit     int
	pause     time.Duration
}

type localUser struct {
	ID     int64
	Email  string
	Status string
}

type summary struct {
	scanned       int
	alreadyLinked int
	matched       int
	created       int
	skipped       int
	failed        int
}

func main() {
	if err := run(); err != nil {
		fmt.Fprintf(os.Stderr, "迁移失败: %v\n", err)
		os.Exit(1)
	}
}

func run() error {
	var opts options
	flag.BoolVar(&opts.apply, "apply", false,
		"真正写入 Sub2API 与本地映射；缺省只做 dry-run")
	flag.StringVar(&opts.onlyEmail, "only", "",
		"只处理指定邮箱，用于小范围试水")
	flag.IntVar(&opts.limit, "limit", 0, "最多处理多少个用户（0 表示不限）")
	flag.DurationVar(&opts.pause, "pause", 100*time.Millisecond,
		"每个用户之间的间隔，避免打爆账号中心")
	flag.Parse()

	databaseURL := os.Getenv("CCHAVEN_DATABASE_URL")
	if databaseURL == "" {
		return errors.New("缺少 CCHAVEN_DATABASE_URL")
	}
	client, err := newSub2APIAdmin()
	if err != nil {
		return err
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	conn, err := pgx.Connect(ctx, databaseURL)
	if err != nil {
		return fmt.Errorf("连接数据库失败: %w", err)
	}
	defer func() { _ = conn.Close(ctx) }()

	users, err := loadUsers(ctx, conn, opts)
	if err != nil {
		return err
	}

	mode := "DRY-RUN（不写任何一侧）"
	if opts.apply {
		mode = "APPLY（会在账号中心建号并写本地映射）"
	}
	fmt.Printf("模式: %s\n待处理用户: %d\n账号中心: %s\n\n", mode, len(users), client.base)

	var stats summary
	for _, user := range users {
		stats.scanned++
		if err := migrateOne(ctx, conn, client, user, opts, &stats); err != nil {
			stats.failed++
			// 单个用户失败不中断整体：把它记下来，人工处理完再重跑（本工具幂等）。
			fmt.Printf("  [失败] %s (#%d): %v\n", mask(user.Email), user.ID, err)
		}
		if opts.pause > 0 {
			time.Sleep(opts.pause)
		}
	}

	fmt.Printf("\n汇总: 扫描 %d | 已有映射 %d | 按邮箱匹配 %d | 新建 %d | 跳过 %d | 失败 %d\n",
		stats.scanned, stats.alreadyLinked, stats.matched, stats.created, stats.skipped, stats.failed)
	if !opts.apply {
		fmt.Println("这是 dry-run，什么都没有写。确认报告无误后再加 -apply 重跑。")
	}
	if stats.failed > 0 {
		return fmt.Errorf("有 %d 个用户未能完成迁移", stats.failed)
	}
	return nil
}

func migrateOne(
	ctx context.Context, conn *pgx.Conn, client *sub2apiAdmin,
	user localUser, opts options, stats *summary,
) error {
	linked, err := hasIdentity(ctx, conn, user.ID)
	if err != nil {
		return err
	}
	if linked {
		stats.alreadyLinked++
		return nil
	}
	if user.Email == "" {
		stats.skipped++
		fmt.Printf("  [跳过] #%d 没有邮箱，无法定位账号中心用户\n", user.ID)
		return nil
	}

	remoteID, err := client.lookup(ctx, user.Email)
	if err != nil {
		return err
	}

	switch {
	case remoteID != "":
		stats.matched++
		fmt.Printf("  [匹配] %s (#%d) → Sub2API %s\n", mask(user.Email), user.ID, remoteID)
	case !opts.apply:
		stats.created++
		fmt.Printf("  [将新建] %s (#%d)\n", mask(user.Email), user.ID)
		return nil
	default:
		remoteID, err = client.create(ctx, user.Email)
		if err != nil {
			return err
		}
		stats.created++
		fmt.Printf("  [新建] %s (#%d) → Sub2API %s\n", mask(user.Email), user.ID, remoteID)
	}

	if !opts.apply {
		return nil
	}
	return linkIdentity(ctx, conn, remoteID, user)
}

func loadUsers(ctx context.Context, conn *pgx.Conn, opts options) ([]localUser, error) {
	query := `
		SELECT u.id, u.email, u.status
		  FROM users u
		  LEFT JOIN sub2api_identities i ON i.user_id = u.id
		 WHERE i.user_id IS NULL`
	args := []any{}
	if opts.onlyEmail != "" {
		query += ` AND lower(u.email) = lower($1)`
		args = append(args, opts.onlyEmail)
	}
	query += ` ORDER BY u.id`
	if opts.limit > 0 {
		query += fmt.Sprintf(" LIMIT %d", opts.limit)
	}

	rows, err := conn.Query(ctx, query, args...)
	if err != nil {
		return nil, fmt.Errorf("读取用户失败: %w", err)
	}
	defer rows.Close()

	var out []localUser
	for rows.Next() {
		var u localUser
		if err := rows.Scan(&u.ID, &u.Email, &u.Status); err != nil {
			return nil, err
		}
		out = append(out, u)
	}
	return out, rows.Err()
}

func hasIdentity(ctx context.Context, conn *pgx.Conn, userID int64) (bool, error) {
	var exists bool
	err := conn.QueryRow(ctx,
		`SELECT EXISTS (SELECT 1 FROM sub2api_identities WHERE user_id = $1)`, userID).Scan(&exists)
	return exists, err
}

// linkIdentity 与运行时的 store.LinkIdentity 写同样的两处，保持数据形状一致。
func linkIdentity(ctx context.Context, conn *pgx.Conn, remoteID string, user localUser) error {
	tx, err := conn.Begin(ctx)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback(ctx) }()

	if _, err := tx.Exec(ctx, `
		INSERT INTO sub2api_identities (sub2api_user_id, user_id, email)
		VALUES ($1, $2, $3)
		ON CONFLICT (sub2api_user_id) DO UPDATE SET email = EXCLUDED.email`,
		remoteID, user.ID, user.Email); err != nil {
		return fmt.Errorf("写入映射失败: %w", err)
	}
	if _, err := tx.Exec(ctx,
		`UPDATE users SET sub2api_user_id = $2 WHERE id = $1`, user.ID, remoteID); err != nil {
		return fmt.Errorf("回写 users.sub2api_user_id 失败: %w", err)
	}
	return tx.Commit(ctx)
}

// —— Sub2API 管理端客户端 ——

type sub2apiAdmin struct {
	base       string
	lookupPath string
	createPath string
	token      string
	http       *http.Client
}

func newSub2APIAdmin() (*sub2apiAdmin, error) {
	token := os.Getenv("CCHAVEN_SUB2API_ADMIN_TOKEN")
	if token == "" {
		return nil, errors.New("缺少 CCHAVEN_SUB2API_ADMIN_TOKEN（只从环境变量读取）")
	}
	return &sub2apiAdmin{
		base:       strings.TrimRight(envOr("CCHAVEN_SUB2API_BASE", "https://api.lumio.games"), "/"),
		lookupPath: envOr("CCHAVEN_SUB2API_ADMIN_LOOKUP_PATH", defaultLookupPath),
		createPath: envOr("CCHAVEN_SUB2API_ADMIN_CREATE_PATH", defaultCreatePath),
		token:      token,
		http:       &http.Client{Timeout: requestTimeout},
	}, nil
}

// lookup 按邮箱查账号中心用户，未命中返回空串。
func (c *sub2apiAdmin) lookup(ctx context.Context, email string) (string, error) {
	endpoint := fmt.Sprintf("%s%s?email=%s", c.base, c.lookupPath, url.QueryEscape(email))
	resp, err := c.do(ctx, http.MethodGet, endpoint, nil)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()

	switch resp.StatusCode {
	case http.StatusOK:
		return decodeID(resp.Body)
	case http.StatusNotFound:
		return "", nil
	default:
		return "", fmt.Errorf("查询账号中心失败: HTTP %d", resp.StatusCode)
	}
}

// create 在账号中心建号；邮箱已存在（409）时回头再查一次，保证幂等。
func (c *sub2apiAdmin) create(ctx context.Context, email string) (string, error) {
	body, err := json.Marshal(map[string]any{
		"email":          email,
		"email_verified": true,
		"source":         "cchaven-migration",
	})
	if err != nil {
		return "", err
	}

	resp, err := c.do(ctx, http.MethodPost, c.base+c.createPath, body)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()

	switch resp.StatusCode {
	case http.StatusOK, http.StatusCreated:
		return decodeID(resp.Body)
	case http.StatusConflict:
		return c.lookup(ctx, email)
	default:
		return "", fmt.Errorf("创建账号中心用户失败: HTTP %d", resp.StatusCode)
	}
}

func (c *sub2apiAdmin) do(ctx context.Context, method, endpoint string, body []byte) (*http.Response, error) {
	var reader io.Reader
	if body != nil {
		reader = bytes.NewReader(body)
	}
	req, err := http.NewRequestWithContext(ctx, method, endpoint, reader)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+c.token)
	req.Header.Set("Accept", "application/json")
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	return c.http.Do(req)
}

// decodeID 从 {"data":{"id":…}} 或裸对象里取出用户 ID，兼容字符串与数字。
func decodeID(r io.Reader) (string, error) {
	raw, err := io.ReadAll(io.LimitReader(r, 1<<20))
	if err != nil {
		return "", err
	}

	var envelope struct {
		Data json.RawMessage `json:"data"`
	}
	payload := raw
	if err := json.Unmarshal(raw, &envelope); err == nil && len(envelope.Data) > 0 {
		payload = envelope.Data
	}

	var body struct {
		ID json.RawMessage `json:"id"`
	}
	if err := json.Unmarshal(payload, &body); err != nil {
		return "", fmt.Errorf("账号中心返回无法解析: %w", err)
	}

	id := strings.Trim(strings.TrimSpace(string(body.ID)), `"`)
	if id == "" || id == "null" {
		return "", errors.New("账号中心返回里没有用户 id")
	}
	return id, nil
}

func envOr(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}

// mask 打码邮箱：迁移日志可能被贴进工单，不该把用户邮箱整个抄进去。
func mask(email string) string {
	name, domain, ok := strings.Cut(email, "@")
	if !ok || len(name) == 0 {
		return "***"
	}
	if len(name) <= 2 {
		return name[:1] + "***@" + domain
	}
	return name[:1] + "***" + name[len(name)-1:] + "@" + domain
}
