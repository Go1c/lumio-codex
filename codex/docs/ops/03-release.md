# 03 · 版本发布与更新提醒

## 1. 版本号要改哪里

客户端版本必须一致（当前示例 `1.2.46`）：

| 位置 | 字段 |
|------|------|
| 根 [`Cargo.toml`](../../Cargo.toml) workspace | `version`（若 workspace 统一管理） |
| [`apps/codex-plus-manager/package.json`](../../apps/codex-plus-manager/package.json) | `"version"` |
| [`apps/codex-plus-manager/src-tauri/tauri.conf.json`](../../apps/codex-plus-manager/src-tauri/tauri.conf.json) | `"version"` |
| `apps/codex-plus-manager/src-tauri/Cargo.toml` | 通常 `version.workspace = true`，跟 workspace |

发版前：

```bash
rg -n '"version"|^version' Cargo.toml apps/codex-plus-manager/package.json apps/codex-plus-manager/src-tauri/tauri.conf.json
```

约定：**Git tag = `v` + 上述版本**，例如 `v1.2.46`。

## 2. 内部未签名发布（当前正式可用通道）

适用：受控内测、渠道包、尚未拿到平台签名证书时。

**主分发：** S3 CDN（`https://s3.lumio.games/lumio-codex/releases/`）  
**回退：** 同批文件挂到 GitHub Release（`prerelease`）

### 2.0 一次性：GitHub Secrets

在 `Go1c/lumio-codex` → Settings → Secrets（或本地 `gh secret set`）：

| Secret | 示例 |
|--------|------|
| `S3_ENDPOINT` | `https://s3.lumio.games` |
| `S3_ACCESS_KEY_ID` | （RustFS Access Key） |
| `S3_SECRET_ACCESS_KEY` | （RustFS Secret） |
| `S3_BUCKET` | `lumio-codex` |
| `S3_PUBLIC_BASE` | `https://s3.lumio.games/lumio-codex/releases` |

### 2.1 自动：打 tag → 构建 → S3 + GitHub prerelease

```bash
# 版本号已对齐后
git tag -a v1.2.46 -m "BestCodex 1.2.46 internal-unsigned"
git push origin v1.2.46
```

Actions → **Internal unsigned build artifacts** 会在三平台构建成功后跑 **Publish S3 + GitHub prerelease**：

- 上传到 `s3://lumio-codex/releases/<version>/`（含 `SHA256SUMS.txt`）  
- 写指针 `releases/latest-internal.json`  
- 创建/更新 tag 对应的 **prerelease**（附件与 CDN 同文件）  

也可在 `publish` 上 **Run workflow**，勾选 `publish`：版本号为 `0.0.0-internal-<run>`，Release tag 为 `internal-run-<run>`。

**`git push origin publish` 同样会走本节的 S3 + 指针 + prerelease**（版本号 `0.0.0-internal-<run>`）。要 semver 文件名：打 `v*` tag。

官网 Codex 站同源 `/latest-internal.json` 不会被本 workflow 改写，发版后还需把 S3 指针拷到站点根目录（见统一官网手册 §4）。

### 2.2 只构建、不分发

1. 不勾选 `publish` 的 `workflow_dispatch`（任意分支）  
2. Actions → **Internal unsigned build artifacts**  
3. 下载四个 artifact 手工分发；告知「未签名 / 右键打开 / SmartScreen」  

不要用「只推 `publish`、不发布」当只构建——`publish` 的 push 会自动上架。

### 2.3 用本地打包 + 手工上传

1. 按 [01](./01-local-build.md) 打出四平台包  
2. 计算 SHA-256 并保存到发布说明  
3. 上传到 S3 `releases/<version>/` 与/或 GitHub Release（可标 `prerelease`）

## 3. 创建 GitHub Release（驱动 App「更新提醒」）

客户端启动后会请求：

```text
GET https://api.github.com/repos/Go1c/lumio-codex/releases/latest
```

解析 `tag_name`，用 semver 与本地 `CARGO_PKG_VERSION` 比较；更高则首页提示「查看更新」，默认打开该 Release 的 `html_url`（或仓库 Releases 页）。

### 3.1 推荐步骤

```bash
# 1. 工作区干净、版本号已对齐、测试已过
git status
# 2. 提交版本 bump + 变更说明（可同步改 CHANGELOG.md）
git tag -a v1.2.46 -m "BestCodex 1.2.46"
git push origin publish
git push origin v1.2.46
```

然后在 GitHub → Releases → Draft a new release：

- Tag：`v1.2.46`  
- Title：`BestCodex 1.2.46`  
- 勾选是否 Pre-release（内测建议勾选）  
- 上传四个 `-internal-unsigned` 制品（或签名后的正式名）  
- 正文写清：平台、未签名须知、SHA-256、已知问题  

**注意：** `releases/latest` 只会指向**最新的非 draft、且通常非 pre-release** 的 Release（GitHub 规则）。若只发 Pre-release，已安装旧版的用户可能**收不到**「有更新」。内测若要提醒生效，请发正式 Release，或接受改客户端去读 `/releases` 列表（当前未做）。

### 3.2 验收更新提醒

1. 安装**低于** tag 的旧包  
2. 确保能访问 GitHub API  
3. 冷启动 → 首页应出现更新条 → 「查看更新」打开 Release 页  
4. 点「稍后」横幅消失（当次会话）  

## 4. 公开签名发布（门槛未打开）

[`.github/workflows/release-assets.yml`](../../.github/workflows/release-assets.yml) 名称为 **Public release gate**，当前逻辑是 **直接失败**，文案要求齐备：

- Apple Developer ID 签名 + 公证  
- Windows 代码签名  
- 受保护的 CI 凭据  
- S3 更新基址（与架构规格双源清单一致）  
- 回滚演练  

在门槛关闭前：

- 不要把 `-internal-unsigned` 宣传为「正式稳定版」  
- 不要关闭系统安全机制扩大分发  
- 正式包命名应去掉 `internal-unsigned` 后缀（签名流程落地时一并改 CI）  

开启时另开变更：改 workflow、注入 secrets、产出 `latest.json`（若恢复双源更新安装），并更新本文。

## 5. 发版检查清单（复制用）

- [ ] 版本号三处（+ workspace）一致  
- [ ] `cargo fmt` / `lumio` 测 / manager 测 / `npm test` / `npm run check` 通过  
- [ ] `node .spec/tools/spec-lint.mjs`（若动过 `.spec/`）  
- [ ] 站点 `https://bestcodex.app` 与帮助 `https://bestcodex.app/help` 健康；充值页 `https://api.lumio.games/purchase` 仍可达（[02](./02-website-deploy.md)、[04](./04-backend.md)）。CORS / DNS 仍待运维。  
- [ ] API `https://api.lumio.games/` 健康（[04](./04-backend.md)）  
- [ ] 四平台包已产出并抽测冒烟（[01](./01-local-build.md) §7）  
- [ ] Tag + Release 已发布；更新提醒在旧版上可复现  
- [ ] CHANGELOG / 发布说明已写  
- [ ] 无秘密进仓库  

## 6. 回滚

1. **客户端**：Release 页保留上一版制品；通知用户安装上一 tag；必要时删除或标记最新 Release 为不可用（谨慎）。  
2. **官网**：Git revert `site/` 后重新部署 Pages。  
3. **API**：按 Sub2API 部署仓的回滚规程（镜像 tag / compose 上一版）。  
4. **不要**在未演练时强推「自动降级安装」。  
