# 02 · 生产环境部署

目标：在一台或多台 Linux 主机上跑出可对外服务的
`api.cc.lumiogame.com` + `cc.lumiogame.com` + `admin.cchaven.cn`。

> **账号体系前提**：终端用户的注册 / 登录 / 口令全部在 Lumio 账号中心
> （Sub2API，`api.lumio.games`），控制面只按令牌认人（详见
> [01-architecture.md](./01-architecture.md)「账号体系」）。因此本篇里的
> `CCHAVEN_SUB2API_BASE` 与 `CCHAVEN_PORTAL_URL` 属于必填项，配错会让**全部**
> 终端用户请求 401/503。管理后台的账号与 TOTP 不受影响，仍在本地。

仓库内**没有**现成 Dockerfile / systemd unit / Nginx 配置样板；下面给出可直接落地的
约定与命令，你按自己的主机管理方式（systemd / Docker / K8s）套一层即可。

---

## 0. 上线前检查清单

- [ ] 域名 DNS 已指向主机（A/AAAA 或 CDN），含**新增**的 `cc.lumiogame.com` 与 `api.cc.lumiogame.com`
- [ ] 旧域名 `cchaven.cn` / `api.cchaven.cn` 已配 301 到新域名（存量书签与安装包）
- [ ] 各主机均已配好 TLS（Let’s Encrypt 或云证书）
- [ ] PostgreSQL 14+（推荐 16）已就绪，有独立业务库与备份策略
- [ ] 已生成三枚 ≥32 字节密钥（见下）
- [ ] **Sub2API 可达**：从控制面主机 `curl -H 'Authorization: Bearer <任一有效令牌>' https://api.lumio.games/api/v1/auth/me` 返回 200
- [ ] **Sub2API 侧已为门户域名放行 CORS**（账号中心页面要直接调它）
- [ ] 已准备 SMTP（可后接；未配时邮件只进 `email_outbox`）
- [ ] 已决定 cookie：同站用 `lax`（推荐）；跨 eTLD+1 才用 `none`
- [ ] 已想好首个 `owner` 管理员邮箱与口令
- [ ] **存量用户迁移**：`cmd/migrate-identities` 先 dry-run 核对，再由负责人确认后 `-apply`（见第 10 节）

---

## 1. PostgreSQL

```bash
# 示例：创建角色与库（按你方规范改密码）
createuser -P cchaven
createdb -O cchaven cchaven
```

连接串形态：

```text
postgres://cchaven:<密码>@<host>:5432/cchaven?sslmode=require
```

生产务必：

- 开启备份（每日逻辑备份 + WAL/快照按 SLA）
- 限制仅控制面主机可达
- `sslmode=require`（或更严）

控制面启动时会自动跑 `migrations/*.sql` 到最新版本，**不要**手工改生产库结构。

---

## 2. 编译控制面二进制

在 CI 或跳板机（Linux amd64/arm64，Go 1.26+）：

```bash
cd services/cchaven-control
make build
# 产物：
#   bin/control
#   bin/admin-bootstrap
```

把两个二进制、以及运行所需的环境文件放到服务器（例如 `/opt/cchaven/control/`）。
`migrations/` 已嵌入二进制，无需单独拷 SQL 目录。

---

## 3. 控制面环境变量

在服务器创建 `/opt/cchaven/control/.env`（权限 `0600`，勿提交 git）：

```bash
CCHAVEN_ENV=prod
CCHAVEN_HTTP_ADDR=127.0.0.1:8080
CCHAVEN_PUBLIC_URL=https://cc.lumiogame.com
CCHAVEN_ADMIN_URL=https://admin.cchaven.cn
CCHAVEN_COOKIE_SAMESITE=lax
CCHAVEN_DATABASE_URL=postgres://cchaven:***@db:5432/cchaven?sslmode=require

# —— 身份真源（Lumio 账号中心）——
# 漏配会回落到线上默认值；配错域名则全部终端用户请求 401/503。
CCHAVEN_SUB2API_BASE=https://api.lumio.games
# 统一门户：既是已下线认证接口 410 响应里的指路地址，也是可信来源之一（CORS）。
CCHAVEN_PORTAL_URL=https://lumiogame.com
# 身份校验结果的缓存时长。调大省外部调用，代价是账号中心停用账号后生效更慢。
CCHAVEN_SUB2API_CACHE_TTL=1m

# 各至少 32 字节；生成：openssl rand -base64 48
CCHAVEN_JWT_SECRET=...
CCHAVEN_CODE_PEPPER=...
CCHAVEN_TOTP_KEY=...

CCHAVEN_ACCESS_TOKEN_TTL=15m
CCHAVEN_REFRESH_TOKEN_TTL=1440h
CCHAVEN_ADMIN_SESSION_TTL=12h

# 邮件（可先留空，业务照常，信进 outbox）
CCHAVEN_SMTP_HOST=smtp.example.com
CCHAVEN_SMTP_PORT=587
CCHAVEN_SMTP_USERNAME=...
CCHAVEN_SMTP_PASSWORD=...
CCHAVEN_SMTP_FROM=CC避风港 <no-reply@cchaven.cn>
```

完整注释见 [`services/cchaven-control/.env.example`](../../services/cchaven-control/.env.example)。

### systemd 示例（按需改路径）

```ini
# /etc/systemd/system/cchaven-control.service
[Unit]
Description=CCHaven control plane
After=network-online.target postgresql.service
Wants=network-online.target

[Service]
Type=simple
User=cchaven
WorkingDirectory=/opt/cchaven/control
EnvironmentFile=/opt/cchaven/control/.env
ExecStart=/opt/cchaven/control/bin/control
Restart=on-failure
RestartSec=3
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now cchaven-control
curl -sS http://127.0.0.1:8080/api/v1/health
```

健康检查通过后再挂公网网关。

---

## 4. 反向代理（Nginx 示意）

要点：

1. `api.cc.lumiogame.com` → 反代到 `127.0.0.1:8080`，保留 `Host` / `X-Forwarded-*`
2. `cc.lumiogame.com` → 静态根目录 = CC 产品站的 `dist/`；`/api/` 反代到控制面
3. `admin.cchaven.cn` → 静态根目录 = `apps/admin` 的 `dist/`；`/api/` 反代到控制面
4. 全站 HTTPS；HSTS 按团队策略开启
5. 旧的 `cchaven.cn` / `api.cchaven.cn` 只做 301，不再承载业务

CC 产品站静态段示意：

```nginx
server {
  server_name cc.lumiogame.com;
  root /var/www/cchaven-web;
  index index.html;

  location /api/ {
    proxy_pass http://127.0.0.1:8080;
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;
  }

  location / {
    try_files $uri $uri/ /index.html;
  }
}
```

管理后台同理，把 `root` 换成 admin 的 `dist/`，`server_name` 换成 `admin.cchaven.cn`。

门户 `lumiogame.com` 不经这里的 `/api` 反代：它带着 Sub2API 令牌**跨源**调
`api.cc.lumiogame.com`，靠 `CCHAVEN_PORTAL_URL` 进可信来源拿到 CORS 头。

API 主机可只反代全部路径到控制面，不托管静态文件。

---

## 5. 构建并发布前端静态站

```bash
# CC 产品站（同源 /api，不要写死 API 域名到构建参数）
cd apps/web
npm ci
npm run build
rsync -a --delete dist/ user@host:/var/www/cchaven-web/

# 管理后台
cd apps/admin
npm ci
npm run build
rsync -a --delete dist/ user@host:/var/www/cchaven-admin/
```

生产构建**不含** MSW mock（`import.meta.env.DEV` 会摇掉）。

联调阶段若必须直连 API 域名（不推荐生产），产品站才设
`VITE_API_BASE_URL=https://api.cc.lumiogame.com`，并确认控制面 CORS /
`CCHAVEN_COOKIE_SAMESITE` 与部署形态一致。

> `apps/web` 里的注册 / 登录 / 找回密码页面已经没有后端可用（控制面对应端点返回
> 410），统一官网 `web/` 就位后应整体退役。上线新域名前请确认这些入口已从
> 导航里摘掉，否则用户会点进一个必然失败的表单。

---

## 6. 创建首个管理员

管理后台**没有**自助注册：

```bash
cd /opt/cchaven/control
export CCHAVEN_ADMIN_PASSWORD='至少8位且含字母与数字'
./bin/admin-bootstrap -email ops@cchaven.cn -name 运营管理员 -role owner
```

或在开发机通过 Makefile（读 `.env`）：

```bash
cd services/cchaven-control
CCHAVEN_ADMIN_PASSWORD='...' make admin EMAIL=ops@cchaven.cn
```

然后打开 `https://admin.cchaven.cn`：

1. 邮箱 + 密码登录  
2. 强制完成 TOTP 绑定（`/auth/totp/setup` → `/enable`）  
3. 进入仪表盘  

角色：`owner` / `ops` / `support`（support 只读，含用户明文邮箱详情入口禁用）。

---

## 7. 桌面 APP 与下载页

1. 按 [03-build-package.md](./03-build-package.md) 打出 macOS 安装包  
2. 上传到对象存储或产品站可下载位置  
3. 更新产品站下载页链接  
4. 用户安装后需能访问 `https://api.cc.lumiogame.com` 与 `https://lumiogame.com/authorize`

桌面 release 构建默认连真实控制面与门户，与下面的取值一致；打包机上若要覆盖：

```bash
export CCHAVEN_API_BASE=https://api.cc.lumiogame.com
export CCHAVEN_WEB_BASE=https://cc.lumiogame.com
export CCHAVEN_PORTAL_BASE=https://lumiogame.com
# 切勿在 release 包里打开 CCHAVEN_CONTROL_MOCK
```

授权确认页由门户提供：它必须带着用户的 Sub2API 令牌调
`POST https://api.cc.lumiogame.com/api/v1/oauth/authorize`（查询参数原样转发），
并把响应里的 `redirect_to` 交给浏览器。门户没上线这一页，桌面端就登不进来。

（具体注入方式以桌面 `control.rs` / 构建脚本为准；若当前从环境变量读取，在打 release
的 shell 里 export 即可。）

---

## 8. 部署后冒烟

| 步骤 | 期望 |
| --- | --- |
| `GET https://api.cc.lumiogame.com/api/v1/health` | 200 |
| 打开 CC 产品站首页 | 无 CORS 报错；公开配置可读 |
| 在门户注册 / 登录后打开 CC 账户页 | 权益与邀请数据可读（首次访问即在 CC 侧开出影子账号） |
| `POST /api/v1/auth/login` | **410 `auth_migrated`**，`details.portal_url` 指向门户 |
| `POST /api/v1/billing/checkout` | **303**，`Location` = `https://api.lumio.games/purchase` |
| 带一个乱写的 Bearer 调 `/api/v1/me` | 401 |
| 临时把 `CCHAVEN_SUB2API_BASE` 指向不可达地址（预发） | `/api/v1/me` 返回 **503 `identity_unavailable`**，不是 200 |
| 管理后台登录 + TOTP | 进仪表盘（未受身份收口影响） |
| 桌面 APP 浏览器登录 | 门户授权页 → 拿到 refresh，账户菜单可见 |
| 故意漏配 admin URL 的演练（预发） | 启动日志有 `CCHAVEN_ADMIN_URL` 告警 |

---

## 10. 存量用户迁移（一次性，高风险）

迁移前建的账号在 `sub2api_identities` 里还没有映射。补映射的工具是
`cmd/migrate-identities`，它会在账号中心为这些邮箱建号（或认领已存在的账号）。

**这是会写生产数据、并在外部系统创建真实用户的操作，必须先取得负责人确认。**

```bash
export CCHAVEN_DATABASE_URL='postgres://…'
export CCHAVEN_SUB2API_BASE='https://api.lumio.games'
export CCHAVEN_SUB2API_ADMIN_TOKEN='…'   # 只走环境变量，别写进 shell 历史与 unit 文件

# 1) dry-run（默认）：只读、只报告
go run ./cmd/migrate-identities

# 2) 单个账号试水
go run ./cmd/migrate-identities -only someone@example.com -apply

# 3) 全量
go run ./cmd/migrate-identities -apply
```

注意事项：

- 工具**不迁移口令**（本地只有不可逆摘要）。迁移出来的账号没有可用口令，
  用户首次登录要走账号中心的「忘记密码」重设——**这条必须提前公告**。
- 幂等：已有映射的用户直接跳过，中断后重跑安全。
- 邮箱已存在于 Sub2API 时按 ID 认领，不新建、不覆盖上游资料。
- 执行前先备份数据库，并在预发库演练一遍。
- 详细前置条件与 Sub2API 端点契约写在
  [`cmd/migrate-identities/main.go`](../../services/cchaven-control/cmd/migrate-identities/main.go)
  的文件头注释里；契约与账号中心不一致时**先改代码，别改数据**。

---

## 9. 日常运维要点

- **日志**：stdout；用 journald / 容器日志收集。勿打印令牌、密码、TOTP 密钥。  
- **迁移**：随新版本二进制启动自动执行；发版前在预发库跑一遍。  
- **密钥轮换**：`JWT_SECRET` / `CODE_PEPPER` / `TOTP_KEY` 轮换会使既有会话或密文失效，需维护窗口。  
- **备份恢复**：先恢复 PostgreSQL，再启动旧/新二进制；应用无本地状态目录。  
- **水平扩展**：限频在进程内；多实例前需确认限频与 outbox worker 语义（当前按单实例设计）。  

支付渠道目前是 mock；接支付宝/微信前不要对真实用户开放付费路径。
