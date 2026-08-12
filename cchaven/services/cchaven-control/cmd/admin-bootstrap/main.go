// Command admin-bootstrap 创建首个管理后台账号。
//
// 管理员是与普通用户完全隔离的独立体系，没有自助注册入口，只能由本命令创建。
// 创建后首次登录必须完成两步验证注册（/api/admin/v1/auth/totp/setup → /enable）。
package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"os"
	"time"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/config"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/db"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/security"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/store"
)

func main() {
	email := flag.String("email", "", "管理员邮箱")
	name := flag.String("name", "管理员", "显示名称")
	role := flag.String("role", "owner", "角色：owner / ops / support")
	flag.Parse()

	if *email == "" {
		fmt.Fprintln(os.Stderr, "用法: admin-bootstrap -email you@example.com [-name 姓名] [-role owner]")
		os.Exit(2)
	}

	// 口令从环境变量读取，避免出现在命令行历史与进程列表中。
	password := os.Getenv("CCHAVEN_ADMIN_PASSWORD")
	if password == "" {
		fmt.Fprintln(os.Stderr, "请通过环境变量 CCHAVEN_ADMIN_PASSWORD 提供初始口令")
		os.Exit(2)
	}
	if !security.ValidatePassword(password) {
		fmt.Fprintln(os.Stderr, "口令不满足规则：至少 8 位，且需同时包含字母和数字")
		os.Exit(2)
	}

	if err := run(*email, password, *name, *role); err != nil {
		fmt.Fprintln(os.Stderr, "创建失败:", err)
		os.Exit(1)
	}
}

func run(email, password, name, role string) error {
	cfg, err := config.Load()
	if err != nil {
		return err
	}

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	pool, err := db.Connect(ctx, cfg.DatabaseURL)
	if err != nil {
		return err
	}
	defer pool.Close()

	if err := db.Migrate(ctx, pool); err != nil {
		return err
	}

	if _, err := store.GetAdminByEmail(ctx, pool, email); err == nil {
		return fmt.Errorf("管理员 %s 已存在", email)
	} else if !errors.Is(err, store.ErrNotFound) {
		return err
	}

	hash, err := security.NewHasher(security.DefaultArgon2Params()).Hash(password)
	if err != nil {
		return err
	}

	admin, err := store.CreateAdmin(ctx, pool, email, hash, name, role)
	if err != nil {
		return err
	}

	fmt.Printf("已创建管理员 #%d %s（角色 %s）\n", admin.ID, admin.Email, admin.Role)
	fmt.Println("下一步：登录后调用 /api/admin/v1/auth/totp/setup 与 /enable 完成两步验证注册。")
	return nil
}
