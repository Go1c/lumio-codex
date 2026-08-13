-- 身份收口到 Sub2API（Lumio 账号中心）。
--
-- 终端用户的邮箱、口令与账号状态从此只存在于 Sub2API；本库的 users 行退化为
-- 「CC 业务侧的影子账号」，只承载订阅 / 邀请 / 设备 / 订单这些 CC 专属数据。
--
-- 兼容策略（刻意不删任何东西）：
--   * users 的 password_hash / email_verification_codes / password_reset_tokens 等
--     历史列与历史表原样保留，存量数据可查、可审计、可回滚；
--   * 新增 sub2api_identities 作为「Sub2API 用户 ID ↔ 本地 users.id」的唯一映射，
--     业务侧的身份主键就是这里的 sub2api_user_id；
--   * users.sub2api_user_id 是同一事实的冗余列，只为让后台的用户列表 / 导出
--     不必每次都 JOIN 映射表，由 sub2api_identities 的写入路径同步维护。

CREATE TABLE sub2api_identities (
    sub2api_user_id text        PRIMARY KEY,
    user_id         bigint      NOT NULL UNIQUE REFERENCES users (id) ON DELETE CASCADE,
    -- 建立映射时 Sub2API 侧的邮箱快照，用于排查「邮箱在上游被改过」这类问题。
    email           text        NOT NULL DEFAULT '',
    linked_at       timestamptz NOT NULL DEFAULT now(),
    last_seen_at    timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX ix_sub2api_identities_email ON sub2api_identities (lower(email));

ALTER TABLE users ADD COLUMN sub2api_user_id text;

CREATE UNIQUE INDEX ux_users_sub2api_user_id
    ON users (sub2api_user_id) WHERE sub2api_user_id IS NOT NULL;

-- 影子账号没有本地口令。password_hash 是 NOT NULL，因此写入一个永远无法匹配的
-- 占位值（不是合法的 argon2 编码，校验必定失败），而不是放宽约束。
COMMENT ON COLUMN users.password_hash IS
    '历史遗留：自有口令已迁移到 Sub2API，新账号写入无法匹配的占位值';
COMMENT ON COLUMN users.sub2api_user_id IS
    'Sub2API 用户 ID；权威映射在 sub2api_identities，本列是便于查询的冗余';
