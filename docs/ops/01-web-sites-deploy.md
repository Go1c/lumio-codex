# 01 · 统一官网三站部署

三站都是**纯静态 SPA**：构建产出一个目录，扔给任意静态托管即可，没有服务端渲染、
没有同源后端。三站的运行时外部依赖只有 Sub2API（`https://api.lumio.games`，浏览器直连）
与 Codex 站的发布指针；**当前版本的三站都不调用 CCHaven 控制面**
（`web/apps/` 下没有任何 `api.cc.lumiogame.com` 或 `/api/v1` 调用）。

## 1. 构建

工作目录是 `web/`（npm workspaces，`packages/ui` 与 `packages/auth` 以**源码**被引用，
不需要先单独构建包）。

```bash
cd web
npm ci

# 三站一次性构建（根 package.json 的 build 递归到各工作区，packages 无 build 脚本会跳过）
npm run build

# 或只构建其中一站
npm run build --workspace @lumio/portal
npm run build --workspace @lumio/cc
npm run build --workspace @lumio/codex
```

| 站点 | 工作区包名 | 产物目录 |
| --- | --- | --- |
| 门户 | `@lumio/portal` | `web/apps/portal/dist/` |
| CC 产品站 | `@lumio/cc` | `web/apps/cc/dist/` |
| Codex 产品站 | `@lumio/codex` | `web/apps/codex/dist/` |

每个 app 的 `build` 是 `tsc -b && vite build`，类型不过就不会产出 `dist/`。
构建前建议先跑收口命令：

```bash
cd web
npm run check   # 各工作区 tsc -b
npm test        # 各工作区 vitest run
```

### 构建期环境变量

三站的域名与接口地址都有生产默认值（收敛在 `web/packages/ui/src/config.ts`），
**默认值就是目标拓扑，正常发布不需要传任何变量**。完整清单见
[`web/README.md`](../../web/README.md)「环境变量」。只在下面两种情况需要覆盖：

- 预发环境用另一套域名 → 传 `VITE_ROOT_DOMAIN` / `VITE_PORTAL_URL` / `VITE_CC_URL` /
  `VITE_CODEX_URL` / `VITE_API_BASE_URL`。
- CC 桌面端安装包已上传 → 传 `VITE_CC_DOWNLOAD_ARM_URL`、`VITE_CC_DOWNLOAD_INTEL_URL`、
  `VITE_CC_VERSION`，否则 CC 下载页显示空态（不是坏链接）。

Vite 只在构建时把这些值内联进产物，**改了必须重新构建并重新发布**，不能在服务器上改。

## 2. 分发方案 A：现有 CCHaven nginx 主机

已经有一台跑 `cchaven-control` + nginx 的主机时，最省事的是把三个 `dist/` 各放一个目录。

```bash
cd web
npm ci && npm run build

rsync -a --delete apps/portal/dist/ user@host:/var/www/lumio-portal/
rsync -a --delete apps/cc/dist/     user@host:/var/www/lumio-cc/
rsync -a --delete apps/codex/dist/  user@host:/var/www/lumio-codex/
```

nginx server 段（三段结构相同，差别只在 `server_name`、`root` 与 CC 的 `/api/` 反代）：

```nginx
# 门户：只连 Sub2API，本机没有可反代的后端
server {
  listen 443 ssl;
  server_name lumiogame.com;
  root /var/www/lumio-portal;
  index index.html;

  location / {
    try_files $uri $uri/ /index.html;   # SPA 回退，否则刷新 /login 会 404
  }
}

# CC 产品站：当前同样只连 Sub2API；/api 反代是给「以后要调控制面」留的位置
server {
  listen 443 ssl;
  server_name cc.lumiogame.com;
  root /var/www/lumio-cc;
  index index.html;

  # 现版本 apps/cc 不请求 /api/*，这段可以先不加。
  # 等 CC 站接入控制面（权益 / 邀请 / 设备）时再打开，走同源避免跨站 cookie 问题。
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

# Codex 产品站：纯静态，另需同源发布指针，见第 4 节
server {
  listen 443 ssl;
  server_name codex.lumiogame.com;
  root /var/www/lumio-codex;
  index index.html;

  location = /latest-internal.json {
    add_header Cache-Control "no-store";
  }

  location / {
    try_files $uri $uri/ /index.html;
  }
}
```

> `cc.lumiogame.com` 这一段与 CCHaven 侧手册
> [`cchaven/docs/ops/02-deploy-production.md`](../../cchaven/docs/ops/02-deploy-production.md) §4
> 描述的是同一个 server 段，区别是 `root` 从旧 `cchaven/apps/web` 的 `dist/` 换成
> `web/apps/cc/dist/`。同一台机器上只保留一份真配置，别两边各配一半。

## 3. 分发方案 B：Cloudflare Pages / 对象存储静态托管

三站互不依赖，可以各建一个 Pages 项目（或各一个桶 + CDN）：

| 项目设置 | 门户 | CC | Codex |
| --- | --- | --- | --- |
| Root directory | `web` | `web` | `web` |
| Build command | `npm ci && npm run build --workspace @lumio/portal` | 同左，`@lumio/cc` | 同左，`@lumio/codex` |
| Output directory | `apps/portal/dist` | `apps/cc/dist` | `apps/codex/dist` |
| 自定义域名 | `lumiogame.com` | `cc.lumiogame.com` | `codex.lumiogame.com` |

要点：

- **SPA 回退必须开**：Pages 用 `_redirects` 里的 `/* /index.html 200`；对象存储 + CDN
  把 404 的错误页指到 `index.html` 并返回 200。门户的 `/login`、`/signup`、`/account`、
  `/logout` 与 CC 的 `/pricing`、`/download` 都是前端路由，没有对应的物理文件。
- **CC 站选方案 B 时没有同源 `/api`**：现版本不需要（不调控制面）。等 CC 站接入控制面时，
  要么在 CDN 上加一条 `/api/*` → `https://api.cc.lumiogame.com` 的转发规则，要么改走跨源
  并确认控制面的 `CCHAVEN_PUBLIC_URL` 已配成 CC 站地址（可信来源同时用于 CORS 与同源校验）。
  优先转发规则，跨源方案要额外验证 cookie 与 CORS。
- 三站都要 https 并开启 HSTS（按团队策略）。

## 4. Codex 站的发布指针 `/latest-internal.json`

Codex 下载区按顺序取版本清单（`web/apps/codex/src/lib/releases.ts`）：

1. 同源 `/latest-internal.json`（首选，避开 S3 的 CORS）
2. `https://s3.lumio.games/lumio-codex/releases/latest-internal.json`
3. 两者都失败 → 退回 GitHub Releases 页

所以**每次发内测包后，都要把最新指针复制到 Codex 站根目录**，否则站上显示的还是旧版本：

```bash
# 发布流水线（.github/workflows/pr-build.yml）已把指针写到 S3；站点侧取同一份
curl -fsS https://s3.lumio.games/lumio-codex/releases/latest-internal.json \
  -o /tmp/latest-internal.json
rsync -a /tmp/latest-internal.json user@host:/var/www/lumio-codex/latest-internal.json
```

仓库里 `codex/site/latest-internal.json` 是旧 Pages 站留下的同一份指针文件，
可作为格式参考（`{ channel, version, tag, published_at, commit, assets[] }`），
但**不要**把它当作发布真值手改——真值在 S3，由 CI 生成。

资产名与平台的对应关系在 `releases.ts`：macOS 仍认 `*-macos-*-internal-unsigned.dmg`，
Windows 认 `*-windows-x64-setup.exe` 或 `*-windows-x64-setup-internal-unsigned.exe`（有签名文件名时优先），
改产物命名要同步改那里，否则下载卡片会退化成「暂无该平台包」。

## 4.1 命令行安装脚本

`web/apps/bestcodex/public/install.sh` 与 `install.ps1` 随站点静态发布，落点必须是根目录：

```bash
curl -fsSL https://bestcodex.app/install.sh | sh     # macOS
irm https://bestcodex.app/install.ps1 | iex          # Windows PowerShell
```

它们**不读同源指针**，直接取 S3 那份 `latest-internal.json`（脚本可能在任何机器上跑，
不保证站点已经同步），再按资产 URL 的同级目录取 `SHA256SUMS.txt` 校验。所以：

- 改产物命名 → 除 `releases.ts` 外，这两个脚本也要改。
- CI 不再产出 `SHA256SUMS.txt` → 脚本会**中止安装**（有意为之，不静默降级）。
- 部署后核对：`curl -fsSL https://bestcodex.app/install.sh | BESTCODEX_DRY_RUN=1 sh`
  应打印解析出的版本与包名，不下载任何东西。
- **部署前这条命令是危险的**：2026-08-17 实测线上仍带 `/* → index.html 200` 兜底，
  `/install.sh` 返回的是 `content-type: text/html` 的首页，管道会把一段 HTML 喂给 `sh`。
  所以对外公布安装命令之前，先确认 `curl -sI https://bestcodex.app/install.sh` 的
  content-type 不是 `text/html`。

用 Homebrew cask 暂时不行：Homebrew 自 2026-09-01 起停止支持通不过 Gatekeeper 的 cask，
未签名公证的包连自建 tap 也上不了。签名闸门开了再评估。

## 5. 发布节奏

三站没有互相依赖，可以分别发。建议顺序：**先产品站、后门户**——产品站的账号入口只是
跳链接，门户没上线时点进去是旧页面；反过来门户先上线而产品站还是旧域名，用户会在
新旧两套导航之间来回跳。

回滚就是重新发布上一份 `dist/`（或在托管平台回滚到上一次部署），静态站没有数据迁移。
