// Package mailer 消费 email_outbox 并投递邮件。
//
// 业务事务只负责把邮件入队，投递由本包的后台 worker 完成。这样注册与找回密码
// 不会被 SMTP 抖动拖垮，测试也可以直接断言发件箱内容而无需 SMTP 服务器。
package mailer

import (
	"context"
	"crypto/tls"
	"fmt"
	"log/slog"
	"net"
	"net/smtp"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/config"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/db"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/store"
)

// sendTimeout 限定单封邮件的整个投递过程。此前 smtp.SendMail 没有任何超时，
// SMTP 挂起会把行锁、连接与整条 worker goroutine 一起无限期拖死（QA S-8）。
const sendTimeout = 15 * time.Second

// Sender 投递单封邮件。实现必须尊重 ctx 的取消与截止时间。
type Sender interface {
	Send(ctx context.Context, to, subject, body string) error
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
			if err := w.DrainOnce(ctx); err != nil {
				slog.Error("邮件投递批次失败", "error", err)
			}
		}
	}
}

// DrainOnce 处理一批待发邮件，供周期调度与测试直接驱动。
func (w *Worker) DrainOnce(ctx context.Context) error {
	// 阶段一：领取并立即提交，行锁与池连接就此释放。
	var messages []store.OutboxMessage
	if err := db.InTx(ctx, w.pool, func(tx pgx.Tx) error {
		claimed, err := store.ClaimPendingEmails(ctx, tx, w.batch)
		messages = claimed
		return err
	}); err != nil {
		return err
	}

	// 阶段二：事务外逐封投递。SMTP 挂起最多占用 sendTimeout，不再持有任何行锁。
	for _, m := range messages {
		subject, body := Render(m.Template, m.Payload)

		sendCtx, cancel := context.WithTimeout(ctx, sendTimeout)
		sendErr := w.sender.Send(sendCtx, m.To, subject, body)
		cancel()

		if sendErr != nil {
			slog.Warn("邮件投递失败", "template", m.Template, "error", sendErr)
			if err := store.MarkEmailFailed(ctx, w.pool, m.ID, sendErr.Error()); err != nil {
				return err
			}
			continue
		}
		if err := store.MarkEmailSent(ctx, w.pool, m.ID, time.Now().UTC()); err != nil {
			return err
		}
	}
	return nil
}

// LogSender 把邮件打到日志，供本地开发使用。
//
// 出于安全考虑，验证码与重设令牌不会出现在日志中，只提示邮件已生成；
// 本地联调请改用接口响应里的 dev_code / dev_token 字段。
type LogSender struct{}

// Send 实现 Sender。
func (LogSender) Send(_ context.Context, to, subject, _ string) error {
	slog.Info("邮件已生成（未实际投递）", "to", to, "subject", subject)
	return nil
}

// SMTPSender 通过 SMTP 投递邮件。
type SMTPSender struct{ cfg config.SMTPConfig }

// NewSMTPSender 构造 SMTP 发送器。
func NewSMTPSender(cfg config.SMTPConfig) *SMTPSender { return &SMTPSender{cfg: cfg} }

// Send 实现 Sender。
//
// 不用 smtp.SendMail：它没有拨号超时也不接受 ctx。这里手动走同一套流程
// （STARTTLS → AUTH → MAIL/RCPT/DATA），拨号 5 秒、整体受调用方的 ctx 截止约束。
func (s *SMTPSender) Send(ctx context.Context, to, subject, body string) error {
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

	fail := func(err error) error { return fmt.Errorf("mailer: SMTP 投递失败: %w", err) }

	dialer := net.Dialer{Timeout: 5 * time.Second}
	if deadline, ok := ctx.Deadline(); ok {
		dialer.Deadline = deadline
	}
	conn, err := dialer.DialContext(ctx, "tcp", addr)
	if err != nil {
		return fail(err)
	}
	defer conn.Close()

	client, err := smtp.NewClient(conn, s.cfg.Host)
	if err != nil {
		return fail(err)
	}
	defer client.Close()

	// 与 smtp.SendMail 一致：服务器支持 STARTTLS 就先升级，明文链路上不发送凭据。
	if ok, _ := client.Extension("STARTTLS"); ok {
		if err := client.StartTLS(&tls.Config{ServerName: s.cfg.Host}); err != nil {
			return fail(err)
		}
	}
	if auth != nil {
		if err := client.Auth(auth); err != nil {
			return fail(err)
		}
	}
	if err := client.Mail(s.cfg.From); err != nil {
		return fail(err)
	}
	if err := client.Rcpt(to); err != nil {
		return fail(err)
	}
	writer, err := client.Data()
	if err != nil {
		return fail(err)
	}
	if _, err := writer.Write([]byte(message)); err != nil {
		_ = writer.Close()
		return fail(err)
	}
	if err := writer.Close(); err != nil {
		return fail(err)
	}
	if err := client.Quit(); err != nil {
		return fail(err)
	}
	return nil
}
