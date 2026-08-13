# 04 · 版本发布与更新

目标：把「合并到主干 → 构建 → 部署 → 冒烟 → 公告」做成可重复流程。
当前版本号起点：`0.1.0`（workspace / 各 `package.json` / `tauri.conf.json`）。

---

## 1. 版本号约定

采用语义化版本 `MAJOR.MINOR.PATCH`：

| 变更 | 怎么加版本 |
| --- | --- |
| 修 bug、文案、无协议破坏 | PATCH |
| 向后兼容的功能 | MINOR |
| API / 桌面协议 / 迁移不兼容 | MAJOR（发版说明必须含迁移步骤） |

需要**同步改**的位置：

1. 根 `Cargo.toml` → `[workspace.package].version`  
2. `apps/desktop/src-tauri/tauri.conf.json` → `version`  
3. `apps/desktop/package.json` → `version`  
4. `apps/web/package.json` / `apps/admin/package.json` → `version`（前端展示/对账用）  
5. Git tag：`v0.1.0`  

控制面 Go module 可不跟前端同号，但**发布说明里要写清楚控制面镜像/二进制对应的 git SHA**。

---

## 2. 发版前质量门禁（必须全绿）

在准备发版的 commit 上：

```bash
# Rust
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace

# 控制面
cd services/cchaven-control
gofmt -l . | tee /tmp/gofmt.out | wc -l   # 必须为 0 行输出
go vet ./...
go test -race ./internal/...
# 有 Postgres 时：
make test-integration

# 前端
cd apps/web && npm ci && npm test && npm run build
cd apps/admin && npm ci && npm test && npm run build
cd apps/desktop && npm ci && npm test && npm run build
```

GitHub Actions：`rust.yml`、`control.yml` 在 PR/push 上已跑核心部分；发版 commit
必须看到这两条 workflow 成功。

---

## 3. 标准发版流程（线上推送）

### 3.1 准备

1. `dev`（或发版分支）合并完毕，CHANGELOG / 发布说明草稿写好。  
2. 预发环境（staging）部署同一 SHA，跑 [02 §8 冒烟](./02-deploy-production.md)。  
3. 确认迁移在预发库自动执行成功（看控制面启动日志）。  
4. 通知相关人维护窗口（若有密钥轮换或不可逆迁移）。

### 3.2 打 tag

```bash
git checkout main   # 或你们的生产分支
git pull
# 确认版本号文件已改
git tag -a v0.1.0 -m "release: v0.1.0"
git push origin main
git push origin v0.1.0
```

### 3.3 构建产物

按 [03-build-package.md](./03-build-package.md) 在干净环境构建，归档：

```text
release/v0.1.0/
  control-linux-amd64
  admin-bootstrap-linux-amd64
  web-dist.tar.gz
  admin-dist.tar.gz
  CCHaven-0.1.0.dmg          # 名称按实际产物
  fns-agent-linux-amd64      # 若本轮分发
  SHA256SUMS
  BUILD_INFO.txt             # git sha、时间、构建机
```

```bash
shasum -a 256 * > SHA256SUMS
```

### 3.4 部署顺序（生产）

**严格按依赖顺序，便于回滚：**

1. **数据库备份**（部署前快照）  
2. **控制面**滚动/替换二进制 → `systemctl restart cchaven-control` → health  
3. **官网静态** `rsync` web `dist/`  
4. **管理后台静态** `rsync` admin `dist/`  
5. **桌面安装包** 上传 CDN，更新下载页链接  
6. （可选）内部分发新的 `fns-agent`  

前端可先于控制面发布**仅当**确认 API 向后兼容；默认仍先控制面后前端。

### 3.5 生产冒烟

复跑 [02 §8](./02-deploy-production.md)。额外：

- 管理后台：用只读 `support` 账号确认权限未倒挂  
- 桌面：真机装新包，走一遍浏览器登录  
- 观察错误日志 15–30 分钟  

### 3.6 公告

- 对内：发版说明 + SHA + 回滚负责人  
- 对外（若需要）：官网更新说明 / 下载页版本号  

---

## 4. 热修复（hotfix）

1. 从 tag 拉 `hotfix/v0.1.x`  
2. 最小 diff 修复 + 补测试  
3. PATCH 版本号 → 走完整门禁 → 新 tag  
4. **只部署受影响组件**（例如只换 `control`）  
5. 修完后合并回主干，避免分叉  

---

## 5. 回滚

| 组件 | 回滚方式 |
| --- | --- |
| 控制面 | 换回上一版二进制并重启；若新迁移已执行，需按迁移说明正向兼容或 DB 回备份 |
| web / admin | `rsync` 上一版 `dist/` |
| 桌面 | 下载页改回旧包；已安装用户需手动装回或等下一版 |
| agent | 用户机换回旧二进制 |

原则：**迁移尽量只追加、不做破坏性改名**；若必须破坏，发版说明写清「不可回滚到 vX」并备份。

---

## 6. 环境与频道

| 频道 | 用途 | 域名建议 |
| --- | --- | --- |
| local | 开发 | localhost |
| staging | 发版前验证 | `staging-api.` / `staging.` / `staging-admin.` |
| production | 正式 | `api.cchaven.cn` / `cchaven.cn` / `admin.cchaven.cn` |

staging 的 `CCHAVEN_PUBLIC_URL` / `CCHAVEN_ADMIN_URL` 必须指向 staging 前端，
否则 CORS/CSRF 与邮件链接会指到生产。

---

## 7. 尚未自动化（后续可建）

以下**当前靠人工**，排期时可逐步脚本化：

- [ ] 前端 CI（web/admin lint+test+build）  
- [ ] 控制面 Docker 镜像与 GHCR 推送  
- [ ] Tauri 签名/公证流水线  
- [ ] GitHub Release 自动挂载 DMG + SHA256SUMS  
- [ ] agent 交叉编译产物挂到桌面部署阶段  

在自动化落地前，以本文人工流程为准，不要口头约定替代文档。
