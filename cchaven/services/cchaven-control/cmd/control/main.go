// Command control 启动 CC避风港 控制面服务。
package main

import (
	"context"
	"errors"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/api"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/config"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/db"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/mailer"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/payments"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/security"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/service"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/store"
)

func main() {
	if err := run(); err != nil {
		slog.Error("服务启动失败", "error", err)
		os.Exit(1)
	}
}

func run() error {
	cfg, err := config.Load()
	if err != nil {
		return err
	}
	setupLogging(cfg)
	for _, warning := range cfg.Warnings() {
		slog.Warn(warning)
	}

	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer stop()

	pool, err := db.Connect(ctx, cfg.DatabaseURL)
	if err != nil {
		return err
	}
	defer pool.Close()

	// 迁移随服务启动执行，保证代码与表结构永远同版本。
	if err := db.Migrate(ctx, pool); err != nil {
		return err
	}
	slog.Info("数据库迁移完成")

	cipher, err := security.NewCipher(cfg.TOTPSecretKey)
	if err != nil {
		return err
	}

	registry := payments.NewRegistry()
	// M1 只接入 mock 渠道；支付宝与微信按 payments.Provider 接口补充实现即可。
	registry.Register(payments.NewMock(cfg.PublicURL, cfg.JWTSecret))

	svc := service.New(pool, cfg, security.NewHasher(security.DefaultArgon2Params()), cipher, registry)

	go mailer.NewWorker(pool, senderFor(cfg)).Run(ctx)
	go runMaintenance(ctx, svc)

	server := &http.Server{
		Addr:              cfg.HTTPAddr,
		Handler:           api.NewServer(svc, cfg).Routes(),
		ReadHeaderTimeout: 10 * time.Second,
		WriteTimeout:      60 * time.Second,
		IdleTimeout:       120 * time.Second,
	}

	errCh := make(chan error, 1)
	go func() {
		slog.Info("控制面服务已启动", "addr", cfg.HTTPAddr, "env", cfg.Env)
		if err := server.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			errCh <- err
		}
	}()

	select {
	case err := <-errCh:
		return err
	case <-ctx.Done():
		slog.Info("收到退出信号，正在优雅关闭")
	}

	shutdownCtx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	return server.Shutdown(shutdownCtx)
}

func senderFor(cfg config.Config) mailer.Sender {
	if cfg.SMTP.Enabled() {
		return mailer.NewSMTPSender(cfg.SMTP)
	}
	slog.Warn("未配置 SMTP，邮件只入发件箱不实际投递")
	return mailer.LogSender{}
}

// runMaintenance 周期性执行清理任务：过期授权码回收、注销冷静期到期处理、
// 停滞邮件回收（worker 崩溃残留的 sending 行，QA S-8）。
func runMaintenance(ctx context.Context, svc *service.Service) {
	ticker := time.NewTicker(time.Hour)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			if n, err := svc.ExpireDeletedAccounts(ctx); err != nil {
				slog.Error("处理注销冷静期失败", "error", err)
			} else if n > 0 {
				slog.Info("已处理到期的注销申请", "count", n)
			}
			if n, err := store.RequeueStaleSendingEmails(ctx, svc.Pool, time.Now().Add(-10*time.Minute)); err != nil {
				slog.Error("回收停滞邮件失败", "error", err)
			} else if n > 0 {
				slog.Info("已回收停滞的投递中的邮件", "count", n)
			}
		}
	}
}

func setupLogging(cfg config.Config) {
	level := slog.LevelInfo
	if cfg.Env == "dev" {
		level = slog.LevelDebug
	}
	slog.SetDefault(slog.New(slog.NewJSONHandler(os.Stdout, &slog.HandlerOptions{Level: level})))
}
