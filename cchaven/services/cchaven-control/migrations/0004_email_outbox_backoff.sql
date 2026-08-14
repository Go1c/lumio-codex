-- QA S-8：邮件投递两阶段化与失败退避。
--
-- 此前 worker 在「FOR UPDATE SKIP LOCKED 的事务内」同步发信：SMTP 挂起会把
-- 行锁、连接与整条 goroutine 一起拖死；失败也只在 25 秒内连败 5 次即永久 failed。
-- 本次改造：
--   * 新增中间态 'sending'：领取即提交（锁立刻释放），投递在事务外进行，
--     崩溃残留的 sending 行由维护任务回收回 pending；
--   * 新增 last_attempt_at：领取与失败都会刷新它，重试按 attempts 递增退避，
--     不再以 5 秒间隔连续打死同一封邮件。

ALTER TABLE email_outbox
    ADD COLUMN last_attempt_at timestamptz;

ALTER TABLE email_outbox
    DROP CONSTRAINT email_outbox_status_check;

ALTER TABLE email_outbox
    ADD CONSTRAINT email_outbox_status_check
        CHECK (status IN ('pending', 'sending', 'sent', 'failed'));
