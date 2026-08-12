# cchaven-control — CC避风港 控制面服务（M1）

账号、订阅、邀请裂变、支付与运营后台的唯一后端。官网（M2）、桌面 APP（M3）与运营后台（M4）都通过本服务的 HTTP API 工作。

- **正式部署 / 发版 / 编译**：[`docs/ops/`](../../docs/ops/README.md)（生产环境以该目录为准）
- 权威规范：[`docs/design/interaction-design.md`](../../docs/design/interaction-design.md)
- 表结构与 API 清单：[`docs/m1-spec.md`](./docs/m1-spec.md)
- 为什么新建服务而不改造 `fast-note-sync-service`：[`docs/adr-0001-new-service.md`](./docs/adr-0001-new-service.md)

## 技术选型

Go + chi + pgx，手写 SQL，不用 ORM 与代码生成。迁移脚本以 `migrations/*.sql` 随代码发布，服务启动时自动执行到最新版本。口令用 Argon2id，令牌只存 SHA-256 摘要。

## 快速开始

```bash
cp .env.example .env          # 按需修改；dev 环境下密钥可留空
make db-up                    # 启动本地 PostgreSQL（开发库 5432 / 测试库 5433）
make run                      # 启动服务，自动执行迁移
curl localhost:8080/api/v1/health
```

没有 Docker 时，把 `CCHAVEN_DATABASE_URL` 指向任意可用的 PostgreSQL 14+ 实例即可。

### 部署前必须想清楚的两件事

**两个前端来源都要配。** 本系统有两套独立部署的前端：官网 `apps/web`（`cchaven.cn`，配 `CCHAVEN_PUBLIC_URL`）
与管理后台 `apps/admin`（`admin.cchaven.cn`，配 `CCHAVEN_ADMIN_URL`）。交互设计第 7 章要求后台独立入口、
与官网完全隔离，所以它不是官网的子路径，必须单独列为可信来源。漏配 `CCHAVEN_ADMIN_URL` 的后果是后台在生产环境
彻底不可用：CORS 不下发 `Access-Control-Allow-Origin`，浏览器直接拦；即便绕过，禁用用户、退款、改运营配置这些
写操作还会被 CSRF 同源校验判成 403。dev 环境放行 localhost 任意端口，因此本地完全看不出这个问题——
生产漏配时服务启动会打一条 `slog.Warn`。可信来源的唯一出处是 `config.Config.TrustedOrigins`，
新增前端时在那里加一处即可，CORS 与同源校验会同时生效。

**`CCHAVEN_COOKIE_SAMESITE` 按同站与否来选。** 判断依据是控制面与前端的 eTLD+1 是否相同：

| 部署形态 | 取值 | 说明 |
| --- | --- | --- |
| 控制面与前端同站（`cchaven.cn` / `admin.cchaven.cn` / `api.cchaven.cn`，eTLD+1 都是 `cchaven.cn`） | `lax`（默认） | 顶层导航照常带 cookie，跨站表单提交天然带不上，更安全，不要改 |
| 控制面在另一个站点（不同 eTLD+1，如前端托管在 `*.vercel.app`、控制面在自有域名） | `none` | 否则浏览器根本不发送 cookie，登录直接失效 |

配成 `none` 时服务会强制 `Secure=true`（浏览器对 `SameSite=None` 的硬性要求），控制面因此必须走 HTTPS；
在非 prod 环境配 `none` 会收到一条启动告警，提示 cookie 可能被浏览器丢弃。不提供 `strict`：
它会让用户从邮件里点重设密码链接跳回来时也不带 cookie，破坏现有链路。

### 创建首个管理员

管理后台是独立账号体系，没有自助注册入口：

```bash
CCHAVEN_ADMIN_PASSWORD='YourPass123' make admin EMAIL=ops@cchaven.cn
```

创建后首次登录需调用 `/api/admin/v1/auth/totp/setup` 与 `/enable` 完成两步验证注册。

## 测试

```bash
make test-unit          # 无需数据库，秒级
make test-integration   # 需要 PostgreSQL
make test               # 全部
```

集成测试默认连 `docker-compose` 的测试库（`localhost:5433`）。未设置 `CCHAVEN_TEST_DATABASE_URL` 时，会自动用
[embedded-postgres](https://github.com/fergusstrange/embedded-postgres) 拉起一个真实的 PostgreSQL（首次运行下载二进制并缓存到 `.embedded-postgres/`）。

> **Apple Silicon 注意**：macOS 默认的 SysV 共享内存上限过低，会让本机直接运行的 PostgreSQL（包括 embedded-postgres）
> 在 `initdb` 阶段报 `could not create shared memory segment`。两个解决办法，二选一：
>
> ```bash
> # 方案 A：抬高内核参数（重启后失效，可写入 /etc/sysctl.conf 持久化）
> sudo sysctl -w kern.sysv.shmmax=1073741824 kern.sysv.shmall=1048576
>
> # 方案 B：改用容器里的 PostgreSQL，绕开宿主机限制
> colima start && make db-up
> ```

每个测试从干净的数据库开始（TRUNCATE + 重新灌种子数据），共用同一个实例，因此不并行运行。
测试用可控时钟（`env.Advance`）验证验证码过期、重发冷却、锁定解除与订阅到期，不依赖 `time.Sleep`。

### 关键链路端到端测试

`test/referral_e2e_test.go` 覆盖 M1 的验收主线：邀请链接访问 → 注册（邀请码随 cookie 自动带入）→ 邮箱验证 →
首次登录 APP → 发放 30 天试用 + 邀请者延长 7 天，并断言归因状态、账户中心汇总数字与双方通知邮件。
同文件还覆盖「每账号一次试用」、同设备指纹重复领取被拒、奖励天数配 0 时的降级行为。

```bash
make test-integration
go test ./test/ -run TestReferralClosureEndToEnd -v   # 只跑主线
```

## 目录结构

```
cmd/control/           服务入口
cmd/admin-bootstrap/   创建首个管理员
migrations/            SQL 迁移脚本（随代码发布，启动时自动执行）
internal/
  api/                 HTTP 传输层：路由、中间件、handler
  service/             业务逻辑与跨表不变量
  store/               数据访问，手写 SQL
  domain/              领域类型与派生规则（不依赖 DB 与 HTTP）
  security/            Argon2id、随机令牌、JWT、AES-GCM、PKCE
  i18n/                界面文案字典（含 6.2 节固定文案）
  apperr/              API 错误类型
  payments/            支付渠道适配器（M1 只接入 mock）
  mailer/              发件箱消费与邮件模板
  ratelimit/           进程内限频
  testsupport/         集成测试夹具
test/                  集成与端到端测试
```

## 几个值得知道的设计决定

**订阅状态不落库**。`subscriptions` 只存 `kind` 与 `expires_at`，「已订阅 / 试用中 / 未订阅 / 已过期」由它们派生，
杜绝状态字段与时间字段不一致。

**业务不变量交给数据库**。`subscription_events` 上的两个部分唯一索引承担了核心规则：
`UNIQUE(user_id) WHERE type='trial_granted'` 保证「每个账号一生只可享用一次免费试用」，
`UNIQUE(type, ref_type, ref_id)` 保证同一次邀请、同一笔订单只结算一次（支付回调可安全重投）。
应用层在行级锁下先做判断，索引是并发与人为误操作下的最后一道防线。

**access token 每次请求都回查会话族**。这样管理员禁用用户、用户撤销设备、修改密码撤销其他会话都能立即生效，
不必等 15 分钟的 token 自然过期。代价是一次主键查询。

**refresh token 轮换 + 重放检测**。已轮换过的令牌被再次出示即判定为外泄，整个会话族立即撤销。

**邮件先入库再投递**。业务事务只写 `email_outbox`，后台 worker 负责发信。注册链路不会被 SMTP 抖动拖垮，
测试也可以直接断言发件箱内容。

**邀请横幅由服务端裁决**。归因载体 `cch_ref` 是 HttpOnly 的，前端 JS 读不到，
所以首页横幅不能靠前端自己缓存的副本决定显示与否——那份副本不会随 30 天 cookie 过期，
也不会因为邀请码被停用而失效，会让用户看到「注册即享首月免费」却实际拿不到（归因以服务端 cookie 为准）。
`GET /api/v1/invites/current` 就是那个权威答案，与落地页共用同一套 valid / inviter / trial_days 口径，
但**不记 `referral_visits`**：那是「邀请链接被打开」这一次事件，首页每次渲染都问一遍会把访问量刷成假数据。

**6.2 节文案只有一处出处**。`internal/i18n` 是防枚举与限频文案的唯一来源，并由 `i18n_test.go` 逐条锁定，
改动必须先改规范。

## 尚未接入的部分

- 支付宝与微信支付：`payments.Provider` 接口已就绪，M1 只实现了 mock 渠道。
- IP 归属地：`session_families.ip_region` 已建列，等接入 IP 库后填充。
- zh-HK 文案：字典已分层，词条待补，缺失时回落 zh-CN。
