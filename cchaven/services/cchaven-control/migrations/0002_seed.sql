-- 运营配置默认值。前台价格、邀请奖励天数、试用时长一律从这里下发，页面不写死。
INSERT INTO ops_configs (key, value) VALUES
    ('invite.reward_days', '7'::jsonb),
    ('invite.trial_days',  '30'::jsonb),
    ('pricing.monthly',    '{"amount_cents": 6800, "currency": "CNY"}'::jsonb)
ON CONFLICT (key) DO NOTHING;

-- 桌面端 OAuth 客户端：公开客户端，强制 PKCE，无 client secret。
-- 回环回调为主，自定义 scheme 兜底（对应交互设计 3.4 / 5.1）。
INSERT INTO oauth_clients (id, name, redirect_uri_patterns, is_public, scopes) VALUES
    ('cchaven-desktop',
     'CC避风港 macOS',
     ARRAY['http://127.0.0.1:*/callback', 'http://localhost:*/callback', 'cchaven://auth/callback'],
     true,
     ARRAY['profile', 'workspace', 'offline_access'])
ON CONFLICT (id) DO NOTHING;
