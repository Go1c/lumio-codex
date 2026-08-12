# 02 · 生产环境部署

目标：在一台或多台 Linux 主机上跑出可对外服务的
`api.cchaven.cn` + `cchaven.cn` + `admin.cchaven.cn`。

仓库内**没有**现成 Dockerfile / systemd unit / Nginx 配置样板；下面给出可直接落地的
约定与命令，你按自己的主机管理方式（systemd / Docker / K8s）套一层即可。

---

## 0. 上线前检查清单

- [ ] 域名 DNS 已指向主机（A/AAAA 或 CDN）
- [ ] 三主机均已配好 TLS（Let’s Encrypt 或云证书）
- [ ] PostgreSQL 14+（推荐 16）已就绪，有独立业务库与备份策略
- [ ] 已生成三枚 ≥32 字节密钥（见下）
- [ ] 已准备 SMTP（可后接；未配时邮件只进 `email_outbox`）
- [ ] 已决定 cookie：同站用 `lax`（推荐）；跨 eTLD+1 才用 `none`
- [ ] 已想好首个 `owner` 管理员邮箱与口令

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
CCHAVEN_PUBLIC_URL=https://cchaven.cn
CCHAVEN_ADMIN_URL=https://admin.cchaven.cn
CCHAVEN_COOKIE_SAMESITE=lax
CCHAVEN_DATABASE_URL=postgres://cchaven:***@db:5432/cchaven?sslmode=require

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

1. `api.cchaven.cn` → 反代到 `127.0.0.1:8080`，保留 `Host` / `X-Forwarded-*`
2. `cchaven.cn` → 静态根目录 = `apps/web` 的 `dist/`；`/api/` 反代到控制面
3. `admin.cchaven.cn` → 静态根目录 = `apps/admin` 的 `dist/`；`/api/` 反代到控制面
4. 全站 HTTPS；HSTS 按团队策略开启

官网静态段示意：

```nginx
server {
  server_name cchaven.cn;
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

API 主机可只反代全部路径到控制面，不托管静态文件。

---

## 5. 构建并发布前端静态站

```bash
# 官网（同源 /api，不要写死 API 域名到构建参数）
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

联调阶段若必须直连 API 域名（不推荐生产），官网才设
`VITE_API_BASE_URL=https://api.cchaven.cn`，并确认控制面 CORS /
`CCHAVEN_COOKIE_SAMESITE` 与部署形态一致。

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
2. 上传到对象存储或官网可下载位置  
3. 更新官网下载页链接（`apps/web` 下载页文案/链接）  
4. 用户安装后需能访问 `https://api.cchaven.cn` 与 `https://cchaven.cn/authorize`

桌面 release 构建默认连真实控制面；打包机上设置：

```bash
export CCHAVEN_API_BASE=https://api.cchaven.cn
export CCHAVEN_WEB_BASE=https://cchaven.cn
# 切勿在 release 包里打开 CCHAVEN_CONTROL_MOCK
```

（具体注入方式以桌面 `control.rs` / 构建脚本为准；若当前从环境变量读取，在打 release
的 shell 里 export 即可。）

---

## 8. 部署后冒烟

| 步骤 | 期望 |
| --- | --- |
| `GET https://api.cchaven.cn/api/v1/health` | 200 |
| 打开官网首页 | 无 CORS 报错；公开配置可读 |
| 注册 → 邮箱验证 → 登录 | 完整走通（需 SMTP 或查 `email_outbox`） |
| 管理后台登录 + TOTP | 进仪表盘 |
| 桌面 APP 浏览器登录 | 拿到 refresh，账户菜单可见 |
| 故意漏配 admin URL 的演练（预发） | 启动日志有 `CCHAVEN_ADMIN_URL` 告警 |

---

## 9. 日常运维要点

- **日志**：stdout；用 journald / 容器日志收集。勿打印令牌、密码、TOTP 密钥。  
- **迁移**：随新版本二进制启动自动执行；发版前在预发库跑一遍。  
- **密钥轮换**：`JWT_SECRET` / `CODE_PEPPER` / `TOTP_KEY` 轮换会使既有会话或密文失效，需维护窗口。  
- **备份恢复**：先恢复 PostgreSQL，再启动旧/新二进制；应用无本地状态目录。  
- **水平扩展**：限频在进程内；多实例前需确认限频与 outbox worker 语义（当前按单实例设计）。  

支付渠道目前是 mock；接支付宝/微信前不要对真实用户开放付费路径。
