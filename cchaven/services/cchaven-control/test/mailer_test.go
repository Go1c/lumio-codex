package test

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/mailer"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/store"
	"github.com/Go1c/fns-workspace/services/cchaven-control/internal/testsupport"
)

// probeSender 在投递回调里窥探该行在数据库里的当前状态。
type probeSender struct {
	env      *testsupport.Env
	statuses []string
	fail     bool
	calls    int
}

func (p *probeSender) Send(_ context.Context, to, _, _ string) error {
	p.calls++
	var status string
	if err := p.env.Pool.QueryRow(context.Background(),
		`SELECT status FROM email_outbox WHERE to_email = $1 ORDER BY id DESC LIMIT 1`, to,
	).Scan(&status); err != nil {
		return err
	}
	p.statuses = append(p.statuses, status)
	if p.fail {
		return errors.New("smtp down")
	}
	return nil
}

func enqueueOne(t *testing.T, env *testsupport.Env) {
	t.Helper()
	if err := store.EnqueueEmail(t.Context(), env.Pool,
		"user@example.com", store.TemplateVerifyCode,
		map[string]any{"code": "123456", "expires_in": 10}); err != nil {
		t.Fatalf("入队失败: %v", err)
	}
}

// TestMailerClaimsBeforeSending 锁住两阶段投递（QA S-8）：领取必须先于发送提交。
// 若发送仍发生在领取事务内，Sender 看到的行状态会是 pending 且行锁被持有；
// 正确实现里 Sender 看到的是 sending（已提交），锁已释放。
func TestMailerClaimsBeforeSending(t *testing.T) {
	env := testsupport.New(t)
	enqueueOne(t, env)

	sender := &probeSender{env: env}
	if err := mailer.NewWorker(env.Pool, sender).DrainOnce(t.Context()); err != nil {
		t.Fatalf("DrainOnce 失败: %v", err)
	}

	if sender.calls != 1 {
		t.Fatalf("应恰好投递一封, got %d", sender.calls)
	}
	if got := sender.statuses[0]; got != "sending" {
		t.Errorf("投递时行状态 = %q, want sending（领取已提交、发送在事务外）", got)
	}

	var status string
	if err := env.Pool.QueryRow(t.Context(),
		`SELECT status FROM email_outbox WHERE to_email = 'user@example.com'`,
	).Scan(&status); err != nil {
		t.Fatalf("查询发件箱失败: %v", err)
	}
	if status != "sent" {
		t.Errorf("投递成功后状态 = %q, want sent", status)
	}
}

// TestMailerBacksOffAfterFailure 锁住失败退避（QA S-8）：失败的邮件退回 pending，
// 但退避窗口内不会被立刻重领；累计 5 次后置 failed。
func TestMailerBacksOffAfterFailure(t *testing.T) {
	env := testsupport.New(t)
	enqueueOne(t, env)

	sender := &probeSender{env: env, fail: true}
	worker := mailer.NewWorker(env.Pool, sender)

	for range 5 {
		if err := worker.DrainOnce(t.Context()); err != nil {
			t.Fatalf("DrainOnce 失败: %v", err)
		}
		// 退避窗口内再跑一批：不得重领同一封。
		before := sender.calls
		if err := worker.DrainOnce(t.Context()); err != nil {
			t.Fatalf("DrainOnce 失败: %v", err)
		}
		if sender.calls != before {
			t.Fatalf("退避窗口内不应重领: calls %d -> %d", before, sender.calls)
		}
		// 把 last_attempt_at 拨回退避窗口之前，模拟冷却结束。
		if _, err := env.Pool.Exec(t.Context(),
			`UPDATE email_outbox SET last_attempt_at = now() - interval '31 minutes'
			 WHERE to_email = 'user@example.com'`); err != nil {
			t.Fatalf("回拨退避时钟失败: %v", err)
		}
	}

	if sender.calls != 5 {
		t.Fatalf("累计尝试次数 = %d, want 5", sender.calls)
	}
	var status string
	var lastError string
	if err := env.Pool.QueryRow(t.Context(),
		`SELECT status, last_error FROM email_outbox WHERE to_email = 'user@example.com'`,
	).Scan(&status, &lastError); err != nil {
		t.Fatalf("查询发件箱失败: %v", err)
	}
	if status != "failed" {
		t.Errorf("5 次失败后状态 = %q, want failed", status)
	}
	if lastError == "" {
		t.Error("失败原因应写入 last_error")
	}
}

// TestMailerRequeuesStaleSending 锁住崩溃恢复（QA S-8）：worker 在领取后、回写前
// 崩溃会留下停滞的 sending 行，维护任务必须把它们收回 pending。
func TestMailerRequeuesStaleSending(t *testing.T) {
	env := testsupport.New(t)
	enqueueOne(t, env)

	if _, err := env.Pool.Exec(t.Context(), `
		UPDATE email_outbox
		   SET status = 'sending', last_attempt_at = now() - interval '20 minutes'
		 WHERE to_email = 'user@example.com'`); err != nil {
		t.Fatalf("构造停滞 sending 行失败: %v", err)
	}

	n, err := store.RequeueStaleSendingEmails(t.Context(), env.Pool, time.Now().Add(-10*time.Minute))
	if err != nil {
		t.Fatalf("回收停滞邮件失败: %v", err)
	}
	if n != 1 {
		t.Fatalf("回收行数 = %d, want 1", n)
	}

	// 收回后能被正常领取投递。
	sender := &probeSender{env: env}
	if err := mailer.NewWorker(env.Pool, sender).DrainOnce(t.Context()); err != nil {
		t.Fatalf("DrainOnce 失败: %v", err)
	}
	if sender.calls != 1 {
		t.Errorf("回收后应重新投递, calls = %d", sender.calls)
	}
}
