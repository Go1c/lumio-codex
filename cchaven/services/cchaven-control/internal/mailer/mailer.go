// Package mailer 消费 email_outbox 并投递邮件。
//
// 业务事务只负责把邮件入队，投递由本包的后台 worker 完成。这样注册与找回密码
// 不会被 SMTP 抖动拖垮，测试也可以直接断言发件箱内容而无需 SMTP 服务器。
package mailer

import (
	"context"
	"fmt"
	"log/slog"
	"net/smtp"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/config"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/db"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/store"
)

// Sender 投递单封邮件。
type Sender interface {
	Send(to, subject, body string) error
}

// Worker 周期性地把发件箱中的待发邮件投递出去。
type Worker struct {
	pool     *db.Pool
	sender   Sender
	interval time.Duration
	batch    int
}

// NewWorker 构造投递 worker。
func NewWorker(pool *db.Pool, sender Sender) *Worker {
	return &Worker{pool: pool, sender: sender, interval: 5 * time.Second, batch: 20}
}

// Run 阻塞运行直到 ctx 取消。
func (w *Worker) Run(ctx context.Context) {
	ticker := time.NewTicker(w.interval)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			if err := w.drain(ctx); err != nil {
				slog.Error("邮件投递批次失败", "error", err)
			}
		}
	}
}

// drain 处理一批待发邮件。
//
// 取件用 FOR UPDATE SKIP LOCKED，多个实例并行运行也不会重复投递同一封。
func (w *Worker) drain(ctx context.Context) error {
	return db.InTx(ctx, w.pool, func(tx pgx.Tx) error {
		messages, err := store.ClaimPendingEmails(ctx, tx, w.batch)
		if err != nil {
			return err
		}

		for _, m := range messages {
			subject, body := Render(m.Template, m.Payload)

			if err := w.sender.Send(m.To, subject, body); err != nil {
				slog.Warn("邮件投递失败", "template", m.Template, "error", err)
				if err := store.MarkEmailFailed(ctx, tx, m.ID, err.Error()); err != nil {
					return err
				}
				continue
			}
			if err := store.MarkEmailSent(ctx, tx, m.ID, time.Now().UTC()); err != nil {
				return err
			}
		}
		return nil
	})
}

// LogSender 把邮件打到日志，供本地开发使用。
//
// 出于安全考虑，验证码与重设令牌不会出现在日志中，只提示邮件已生成；
// 本地联调请改用接口响应里的 dev_code / dev_token 字段。
type LogSender struct{}

// Send 实现 Sender。
func (LogSender) Send(to, subject, _ string) error {
	slog.Info("邮件已生成（未实际投递）", "to", to, "subject", subject)
	return nil
}

// SMTPSender 通过 SMTP 投递邮件。
type SMTPSender struct{ cfg config.SMTPConfig }

// NewSMTPSender 构造 SMTP 发送器。
func NewSMTPSender(cfg config.SMTPConfig) *SMTPSender { return &SMTPSender{cfg: cfg} }

// Send 实现 Sender。
func (s *SMTPSender) Send(to, subject, body string) error {
	addr := fmt.Sprintf("%s:%d", s.cfg.Host, s.cfg.Port)

	var auth smtp.Auth
	if s.cfg.Username != "" {
		auth = smtp.PlainAuth("", s.cfg.Username, s.cfg.Password, s.cfg.Host)
	}

	message := strings.Join([]string{
		"From: " + s.cfg.From,
		"To: " + to,
		"Subject: " + subject,
		"MIME-Version: 1.0",
		"Content-Type: text/plain; charset=UTF-8",
		"",
		body,
	}, "\r\n")

	if err := smtp.SendMail(addr, auth, s.cfg.From, []string{to}, []byte(message)); err != nil {
		return fmt.Errorf("mailer: SMTP 投递失败: %w", err)
	}
	return nil
}
