-- CC避风港 控制面 初始表结构
-- 约定：时间列一律 timestamptz；枚举用 text + CHECK；令牌只存 sha256 摘要；金额单位为「分」。

--------------------------------------------------------------------------------
-- 身份与账号
--------------------------------------------------------------------------------

-- 用户 ID 对外展示为 U-{id}（原型示例 U-100986），故序列自 100000 起。
CREATE SEQUENCE users_id_seq START WITH 100000;

CREATE TABLE users (
    id                    bigint       PRIMARY KEY DEFAULT nextval('users_id_seq'),
    email                 text         NOT NULL,
    password_hash         text         NOT NULL,
    display_name          text         NOT NULL DEFAULT '',
    status                text         NOT NULL DEFAULT 'pending_email'
                                       CHECK (status IN ('pending_email', 'active', 'disabled')),
    email_verified_at     timestamptz,
    locked_until          timestamptz,
    failed_login_count    integer      NOT NULL DEFAULT 0,
    registration_source   text         NOT NULL DEFAULT 'organic'
                                       CHECK (registration_source IN ('organic', 'invite', 'other')),
    referred_by_user_id   bigint       REFERENCES users (id) ON DELETE SET NULL,
    trial_granted_at      timestamptz,
    deletion_requested_at timestamptz,
    disabled_at           timestamptz,
    disabled_by_admin_id  bigint,
    disabled_reason       text,
    signup_ip             text         NOT NULL DEFAULT '',
    signup_user_agent     text         NOT NULL DEFAULT '',
    last_active_at        timestamptz,
    created_at            timestamptz  NOT NULL DEFAULT now(),
    updated_at            timestamptz  NOT NULL DEFAULT now()
);

ALTER SEQUENCE users_id_seq OWNED BY users.id;

-- 邮箱大小写不敏感唯一：写入前在应用层统一小写，此处再加一道保险。
CREATE UNIQUE INDEX ux_users_email ON users (lower(email));
CREATE INDEX ix_users_status ON users (status);
CREATE INDEX ix_users_referred_by ON users (referred_by_user_id) WHERE referred_by_user_id IS NOT NULL;
CREATE INDEX ix_users_created_at ON users (created_at DESC);

-- 注册验证码与改邮箱验证码共用一张表。
CREATE TABLE email_verification_codes (
    id            bigserial    PRIMARY KEY,
    user_id       bigint       NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    purpose       text         NOT NULL CHECK (purpose IN ('signup', 'email_change')),
    target_email  text         NOT NULL,
    code_hash     text         NOT NULL,
    expires_at    timestamptz  NOT NULL,
    attempts_used integer      NOT NULL DEFAULT 0,
    max_attempts  integer      NOT NULL DEFAULT 5,
    consumed_at   timestamptz,
    last_sent_at  timestamptz  NOT NULL DEFAULT now(),
    created_at    timestamptz  NOT NULL DEFAULT now()
);

-- 同一用户同一用途同时只允许存在一个未消费的验证码；重发即替换。
CREATE UNIQUE INDEX ux_email_codes_active
    ON email_verification_codes (user_id, purpose) WHERE consumed_at IS NULL;

CREATE TABLE password_reset_tokens (
    id           bigserial    PRIMARY KEY,
    user_id      bigint       NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash   text         NOT NULL UNIQUE,
    expires_at   timestamptz  NOT NULL,
    consumed_at  timestamptz,
    requested_ip text         NOT NULL DEFAULT '',
    created_at   timestamptz  NOT NULL DEFAULT now()
);

CREATE INDEX ix_password_reset_user ON password_reset_tokens (user_id);

--------------------------------------------------------------------------------
-- 会话族与 refresh token 轮换
--------------------------------------------------------------------------------

-- 一次登录 = 一个会话族 = 官网「登录设备与授权」列表中的一行。
CREATE TABLE session_families (
    id              uuid         PRIMARY KEY,
    user_id         bigint       NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    client          text         NOT NULL CHECK (client IN ('web', 'app')),
    oauth_client_id text,
    device_name     text         NOT NULL DEFAULT '',
    platform        text         NOT NULL DEFAULT 'browser'
                                 CHECK (platform IN ('browser', 'macos', 'unknown')),
    os_version      text         NOT NULL DEFAULT '',
    arch            text         NOT NULL DEFAULT '',
    app_version     text         NOT NULL DEFAULT '',
    user_agent      text         NOT NULL DEFAULT '',
    ip              text         NOT NULL DEFAULT '',
    ip_region       text         NOT NULL DEFAULT '',
    created_at      timestamptz  NOT NULL DEFAULT now(),
    last_seen_at    timestamptz  NOT NULL DEFAULT now(),
    revoked_at      timestamptz,
    revoked_reason  text         CHECK (revoked_reason IN (
                                     'user_logout', 'user_revoke', 'revoke_others',
                                     'password_change', 'password_reset',
                                     'admin_disable', 'reuse_detected', 'account_deleted'))
);

CREATE INDEX ix_session_families_user ON session_families (user_id, revoked_at);

CREATE TABLE refresh_tokens (
    id             uuid         PRIMARY KEY,
    family_id      uuid         NOT NULL REFERENCES session_families (id) ON DELETE CASCADE,
    token_hash     text         NOT NULL UNIQUE,
    issued_at      timestamptz  NOT NULL DEFAULT now(),
    expires_at     timestamptz  NOT NULL,
    used_at        timestamptz,
    replaced_by_id uuid,
    revoked_at     timestamptz
);

CREATE INDEX ix_refresh_tokens_family ON refresh_tokens (family_id);

--------------------------------------------------------------------------------
-- OAuth 2.0 授权服务器（APP 通过浏览器登录）
--------------------------------------------------------------------------------

CREATE TABLE oauth_clients (
    id                    text         PRIMARY KEY,
    name                  text         NOT NULL,
    redirect_uri_patterns text[]       NOT NULL,
    is_public             boolean      NOT NULL DEFAULT true,
    scopes                text[]       NOT NULL,
    created_at            timestamptz  NOT NULL DEFAULT now()
);

CREATE TABLE oauth_authorization_codes (
    code_hash             text         PRIMARY KEY,
    client_id             text         NOT NULL REFERENCES oauth_clients (id) ON DELETE CASCADE,
    user_id               bigint       NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    redirect_uri          text         NOT NULL,
    scope                 text         NOT NULL,
    code_challenge        text         NOT NULL,
    code_challenge_method text         NOT NULL CHECK (code_challenge_method = 'S256'),
    device_name           text         NOT NULL DEFAULT '',
    platform              text         NOT NULL DEFAULT 'macos',
    os_version            text         NOT NULL DEFAULT '',
    arch                  text         NOT NULL DEFAULT '',
    app_version           text         NOT NULL DEFAULT '',
    expires_at            timestamptz  NOT NULL,
    consumed_at           timestamptz,
    created_at            timestamptz  NOT NULL DEFAULT now()
);

CREATE INDEX ix_oauth_codes_expiry ON oauth_authorization_codes (expires_at);

--------------------------------------------------------------------------------
-- 订阅（单一包月）
--------------------------------------------------------------------------------

-- 每个用户恒有一行；对外状态由 kind + expires_at 派生，不落库，避免状态与时间不一致。
CREATE TABLE subscriptions (
    user_id           bigint       PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    kind              text         CHECK (kind IN ('trial', 'paid')),
    expires_at        timestamptz,
    trial_expires_at  timestamptz,
    bonus_days_total  integer      NOT NULL DEFAULT 0,
    created_at        timestamptz  NOT NULL DEFAULT now(),
    updated_at        timestamptz  NOT NULL DEFAULT now()
);

-- 订阅时长变更总账：任何一次时长变动都必须留痕。
CREATE TABLE subscription_events (
    id                 bigserial    PRIMARY KEY,
    user_id            bigint       NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    type               text         NOT NULL CHECK (type IN (
                                        'trial_granted', 'invite_bonus', 'purchase',
                                        'refund_revoke', 'admin_adjust')),
    days_delta         integer      NOT NULL,
    expires_at_before  timestamptz,
    expires_at_after   timestamptz,
    ref_type           text,
    ref_id             text,
    note               text         NOT NULL DEFAULT '',
    created_at         timestamptz  NOT NULL DEFAULT now()
);

CREATE INDEX ix_subscription_events_user ON subscription_events (user_id, created_at DESC);

-- 「每个账号一生只可享用一次免费试用」的数据库级硬约束。
CREATE UNIQUE INDEX ux_subscription_events_trial
    ON subscription_events (user_id) WHERE type = 'trial_granted';

-- 同一来源（一次邀请、一笔订单、一次退款）只允许结算一次，保证 webhook 重投幂等。
CREATE UNIQUE INDEX ux_subscription_events_ref
    ON subscription_events (type, ref_type, ref_id) WHERE ref_id IS NOT NULL;

--------------------------------------------------------------------------------
-- 订单与付款
--------------------------------------------------------------------------------

-- 订单号 CC{YYYYMMDD}-{6 位序号}，按天连续，用独立序号表避免并发空洞。
CREATE TABLE order_sequences (
    day      date    PRIMARY KEY,
    next_seq bigint  NOT NULL
);

CREATE TABLE orders (
    id               bigserial    PRIMARY KEY,
    order_no         text         NOT NULL UNIQUE,
    user_id          bigint       NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    amount_cents     bigint       NOT NULL CHECK (amount_cents >= 0),
    currency         text         NOT NULL DEFAULT 'CNY',
    channel          text         NOT NULL CHECK (channel IN ('alipay', 'wechat', 'card', 'mock')),
    status           text         NOT NULL DEFAULT 'pending'
                                  CHECK (status IN ('pending', 'paid', 'refunding', 'refunded', 'failed')),
    period_months    integer      NOT NULL DEFAULT 1,
    provider         text         NOT NULL DEFAULT '',
    provider_txn_id  text,
    idempotency_key  text         UNIQUE,
    paid_at          timestamptz,
    created_at       timestamptz  NOT NULL DEFAULT now(),
    updated_at       timestamptz  NOT NULL DEFAULT now()
);

CREATE INDEX ix_orders_user ON orders (user_id, created_at DESC);
CREATE INDEX ix_orders_status ON orders (status, created_at DESC);
CREATE INDEX ix_orders_paid_at ON orders (paid_at DESC) WHERE status = 'paid';

-- 支付回调原始记录，便于对账与排查。
CREATE TABLE payment_events (
    id           bigserial    PRIMARY KEY,
    order_id     bigint       REFERENCES orders (id) ON DELETE SET NULL,
    type         text         NOT NULL,
    provider     text         NOT NULL,
    payload      jsonb        NOT NULL DEFAULT '{}'::jsonb,
    signature_ok boolean      NOT NULL DEFAULT false,
    created_at   timestamptz  NOT NULL DEFAULT now()
);

CREATE INDEX ix_payment_events_order ON payment_events (order_id, created_at DESC);

CREATE TABLE refunds (
    id                    bigserial    PRIMARY KEY,
    order_id              bigint       NOT NULL REFERENCES orders (id) ON DELETE RESTRICT,
    amount_cents          bigint       NOT NULL CHECK (amount_cents >= 0),
    status                text         NOT NULL DEFAULT 'pending'
                                       CHECK (status IN ('pending', 'succeeded', 'failed')),
    requested_by_admin_id bigint,
    provider_refund_id    text,
    reason                text         NOT NULL DEFAULT '',
    created_at            timestamptz  NOT NULL DEFAULT now(),
    completed_at          timestamptz
);

CREATE INDEX ix_refunds_order ON refunds (order_id);

--------------------------------------------------------------------------------
-- 邀请裂变
--------------------------------------------------------------------------------

CREATE TABLE referral_codes (
    code        text         PRIMARY KEY,
    user_id     bigint       NOT NULL UNIQUE REFERENCES users (id) ON DELETE CASCADE,
    disabled_at timestamptz,
    created_at  timestamptz  NOT NULL DEFAULT now()
);

-- 三步闭环第一步：链接访问 → cookie。
CREATE TABLE referral_visits (
    id         bigserial    PRIMARY KEY,
    code       text         NOT NULL,
    visitor_id uuid         NOT NULL,
    ip         text         NOT NULL DEFAULT '',
    user_agent text         NOT NULL DEFAULT '',
    created_at timestamptz  NOT NULL DEFAULT now()
);

CREATE INDEX ix_referral_visits_code ON referral_visits (code, created_at DESC);
CREATE INDEX ix_referral_visits_visitor ON referral_visits (visitor_id);

-- 第二、三步：注册 → 首次 APP 登录。一名被邀请者只归因一次（首次触达胜出）。
CREATE TABLE referral_attributions (
    id                       bigserial    PRIMARY KEY,
    code                     text         NOT NULL,
    inviter_user_id          bigint       NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    invitee_user_id          bigint       NOT NULL UNIQUE REFERENCES users (id) ON DELETE CASCADE,
    visitor_id               uuid,
    stage                    text         NOT NULL DEFAULT 'registered'
                                          CHECK (stage IN ('registered', 'activated')),
    registered_at            timestamptz  NOT NULL DEFAULT now(),
    activated_at             timestamptz,
    trial_granted            boolean      NOT NULL DEFAULT false,
    inviter_bonus_days       integer      NOT NULL DEFAULT 0,
    inviter_bonus_granted_at timestamptz,
    created_at               timestamptz  NOT NULL DEFAULT now(),
    CONSTRAINT ck_referral_no_self CHECK (inviter_user_id <> invitee_user_id)
);

CREATE INDEX ix_referral_attr_inviter ON referral_attributions (inviter_user_id, created_at DESC);

-- 防重复领取试用：同设备 / 同支付资料 / 同注册 IP 指纹一旦用过即不可再用。
CREATE TABLE trial_fingerprints (
    id         bigserial    PRIMARY KEY,
    kind       text         NOT NULL CHECK (kind IN ('device', 'payment', 'signup_ip')),
    value_hash text         NOT NULL,
    user_id    bigint       NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at timestamptz  NOT NULL DEFAULT now(),
    CONSTRAINT ux_trial_fingerprint UNIQUE (kind, value_hash)
);

--------------------------------------------------------------------------------
-- 运营配置与发布
--------------------------------------------------------------------------------

CREATE TABLE ops_configs (
    key                  text         PRIMARY KEY,
    value                jsonb        NOT NULL,
    updated_at           timestamptz  NOT NULL DEFAULT now(),
    updated_by_admin_id  bigint
);

CREATE TABLE app_releases (
    id           bigserial    PRIMARY KEY,
    version      text         NOT NULL,
    channel      text         NOT NULL DEFAULT 'stable',
    arch         text         NOT NULL CHECK (arch IN ('arm64', 'x86_64')),
    download_url text         NOT NULL,
    min_os       text         NOT NULL DEFAULT '',
    released_at  timestamptz  NOT NULL DEFAULT now(),
    is_current   boolean      NOT NULL DEFAULT false,
    CONSTRAINT ux_app_release UNIQUE (version, channel, arch)
);

CREATE INDEX ix_app_releases_current ON app_releases (channel, arch) WHERE is_current;

--------------------------------------------------------------------------------
-- 遥测（支撑后台平台/版本分布、DAU、留存）
--------------------------------------------------------------------------------

CREATE TABLE user_devices (
    id            bigserial    PRIMARY KEY,
    user_id       bigint       NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    device_id     text         NOT NULL,
    platform      text         NOT NULL DEFAULT 'macos',
    os_version    text         NOT NULL DEFAULT '',
    arch          text         NOT NULL DEFAULT '',
    app_version   text         NOT NULL DEFAULT '',
    first_seen_at timestamptz  NOT NULL DEFAULT now(),
    last_seen_at  timestamptz  NOT NULL DEFAULT now(),
    CONSTRAINT ux_user_device UNIQUE (user_id, device_id)
);

CREATE INDEX ix_user_devices_last_seen ON user_devices (last_seen_at DESC);

CREATE TABLE user_activity_days (
    user_id bigint  NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    day     date    NOT NULL,
    PRIMARY KEY (user_id, day)
);

CREATE INDEX ix_user_activity_day ON user_activity_days (day);

--------------------------------------------------------------------------------
-- 管理后台
--------------------------------------------------------------------------------

CREATE TABLE admins (
    id                 bigserial    PRIMARY KEY,
    email              text         NOT NULL UNIQUE,
    password_hash      text         NOT NULL,
    display_name       text         NOT NULL DEFAULT '',
    role               text         NOT NULL DEFAULT 'ops'
                                    CHECK (role IN ('owner', 'ops', 'support')),
    totp_secret_enc    text,
    totp_enabled_at    timestamptz,
    status             text         NOT NULL DEFAULT 'active'
                                    CHECK (status IN ('active', 'disabled')),
    failed_login_count integer      NOT NULL DEFAULT 0,
    locked_until       timestamptz,
    last_login_at      timestamptz,
    created_at         timestamptz  NOT NULL DEFAULT now()
);

CREATE TABLE admin_sessions (
    id         uuid         PRIMARY KEY,
    admin_id   bigint       NOT NULL REFERENCES admins (id) ON DELETE CASCADE,
    token_hash text         NOT NULL UNIQUE,
    mfa_passed boolean      NOT NULL DEFAULT false,
    ip         text         NOT NULL DEFAULT '',
    user_agent text         NOT NULL DEFAULT '',
    expires_at timestamptz  NOT NULL,
    revoked_at timestamptz,
    created_at timestamptz  NOT NULL DEFAULT now()
);

CREATE INDEX ix_admin_sessions_admin ON admin_sessions (admin_id);

-- 7.5：破坏性操作留操作人 + 时间 + 前后值。
CREATE TABLE audit_logs (
    id          bigserial    PRIMARY KEY,
    actor_type  text         NOT NULL CHECK (actor_type IN ('admin', 'user', 'system')),
    actor_id    text         NOT NULL DEFAULT '',
    action      text         NOT NULL,
    target_type text         NOT NULL DEFAULT '',
    target_id   text         NOT NULL DEFAULT '',
    before      jsonb,
    after       jsonb,
    ip          text         NOT NULL DEFAULT '',
    user_agent  text         NOT NULL DEFAULT '',
    created_at  timestamptz  NOT NULL DEFAULT now()
);

CREATE INDEX ix_audit_logs_created ON audit_logs (created_at DESC);
CREATE INDEX ix_audit_logs_actor ON audit_logs (actor_type, actor_id, created_at DESC);

--------------------------------------------------------------------------------
-- 邮件发件箱（可靠投递；测试断言此表，不依赖 SMTP）
--------------------------------------------------------------------------------

CREATE TABLE email_outbox (
    id         bigserial    PRIMARY KEY,
    to_email   text         NOT NULL,
    template   text         NOT NULL,
    payload    jsonb        NOT NULL DEFAULT '{}'::jsonb,
    status     text         NOT NULL DEFAULT 'pending'
                            CHECK (status IN ('pending', 'sent', 'failed')),
    attempts   integer      NOT NULL DEFAULT 0,
    last_error text         NOT NULL DEFAULT '',
    created_at timestamptz  NOT NULL DEFAULT now(),
    sent_at    timestamptz
);

CREATE INDEX ix_email_outbox_pending ON email_outbox (created_at) WHERE status = 'pending';
