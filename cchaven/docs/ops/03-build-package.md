# 03 · 本地编译与打包

工具链版本以仓库锁定为准：

| 栈 | 版本 |
| --- | --- |
| Rust | 1.94（`Cargo.toml` `rust-version`） |
| Go | 1.26（CI `control.yml`） |
| Node | 建议 20 LTS 或 22 LTS |

---

## 1. 一次性准备

```bash
# Rust
rustup toolchain install 1.94
rustup default 1.94
rustup component add rustfmt clippy
# 桌面打包另需：https://v2.tauri.app/start/prerequisites/

# Go
# 安装 Go 1.26+，并确保 `go version` 符合

# Node
# 安装 Node 20+，启用 corepack 可选
```

克隆仓库后：

```bash
cd /path/to/fns-workspace
```

---

## 2. 控制面（Go）

```bash
cd services/cchaven-control
cp .env.example .env          # 本地可留空密钥（仅 CCHAVEN_ENV=dev）
make db-up                    # 需要 Docker；或自备 Postgres
make run                      # http://localhost:8080
make build                    # → bin/control, bin/admin-bootstrap
make test-unit
# 集成测试（本机 Postgres / Docker 测试库 / CI）：
make test-integration
```

Apple Silicon 上若 `shmget` 失败，见控制面 README 的 sysctl / colima 说明。

生产交叉编译示例（在 macOS 上打 Linux amd64）：

```bash
cd services/cchaven-control
GOOS=linux GOARCH=amd64 CGO_ENABLED=0 go build -o bin/control-linux-amd64 ./cmd/control
GOOS=linux GOARCH=amd64 CGO_ENABLED=0 go build -o bin/admin-bootstrap-linux-amd64 ./cmd/admin-bootstrap
```

---

## 3. 官网与管理后台（前端静态站）

```bash
# 官网
cd apps/web
npm ci
npm run lint
npm test
npm run build                 # → dist/

# 管理后台
cd apps/admin
npm ci
npm run lint
npm test
npm run build                 # → dist/
```

开发默认 MSW mock；连本地控制面：

```bash
# web
VITE_ENABLE_MSW=false VITE_API_BASE_URL=http://localhost:8080 npm run dev

# admin（Vite 把 /api 代理到控制面，cookie 同源）
VITE_USE_MOCK=false VITE_API_ORIGIN=http://localhost:8080 npm run dev
```

---

## 4. Rust workspace（协议 / 同步引擎 / agent）

在仓库根：

```bash
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
```

只编 agent：

```bash
cargo build --locked -p fns-agent --release
# → target/release/fns-agent
```

### Linux agent 交叉编译（给用户云主机）

目标：在用户 Ubuntu/Debian x86_64 上跑的 daemon。

```bash
# 需安装对应 target 与链接器（以本机工具链为准）
rustup target add x86_64-unknown-linux-gnu
# 若在 macOS 上交叉，通常还要 cross 或 Linux 容器；推荐在 ubuntu-latest CI 里编：
cargo build --locked -p fns-agent --release --target x86_64-unknown-linux-gnu
```

产物：`target/x86_64-unknown-linux-gnu/release/fns-agent`

配置文件 schema：`fns-agent-config/1`（见 `bins/fns-agent/src/config.rs`）。
token 在独立 `0600` 文件，不进 JSON。

> 桌面向导**尚未**自动上传该二进制（spec-gaps B2）。上线阶段可在官网文档提供
> 「手动 scp + systemd」步骤，或后续补部署流水线。

---

## 5. 桌面 APP（Tauri 2 / macOS）

```bash
cd apps/desktop
npm ci
npm run typecheck
npm test
npm run build                 # 仅前端 dist

# 开发（需 Rust + Tauri 前置）
cargo tauri dev               # 或安装 @tauri-apps/cli 后 npx tauri dev

# 打 release 安装包
CCHAVEN_CONTROL_MOCK=0 \
CCHAVEN_API_BASE=https://api.cchaven.cn \
CCHAVEN_WEB_BASE=https://cchaven.cn \
cargo tauri build
```

产物一般在：

```text
apps/desktop/src-tauri/target/release/bundle/
  macos/CC避风港.app
  dmg/….dmg
```

（具体子目录以本机 Tauri 2 输出为准。）

bundle 标识：`cn.cchaven.desktop`；版本与 `src-tauri/tauri.conf.json` /
workspace `version` 对齐。

签名与公证（Apple Developer）：按苹果与 Tauri 文档配置证书、`APPLE_ID` 等；
本仓库暂未内置签名脚本，正式分发前必须在打包机配好，否则用户无法顺利打开。

---

## 6. 推荐的「一键本地验收」顺序

```bash
# 根目录 — Rust
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace

# 控制面
cd services/cchaven-control && make test-unit && cd ../..

# 前端
cd apps/web && npm ci && npm test && npm run build && cd ../..
cd apps/admin && npm ci && npm test && npm run build && cd ../..
cd apps/desktop && npm ci && npm test && npm run build && cd ../..
```

CI 已覆盖：

- `.github/workflows/rust.yml` — fmt / clippy / test  
- `.github/workflows/control.yml` — gofmt / vet / unit + Postgres 集成测试  

前端与桌面打包目前靠本地或自建流水线触发。

---

## 7. 产物清单（发版时归档）

| 产物 | 来源命令 | 部署去向 |
| --- | --- | --- |
| `control` + `admin-bootstrap` | `make build` | API 主机 |
| `apps/web/dist/` | `npm run build` | `cchaven.cn` 静态根 |
| `apps/admin/dist/` | `npm run build` | `admin.cchaven.cn` 静态根 |
| `CC避风港.app` / `.dmg` | `cargo tauri build` | 下载 CDN / 官网 |
| `fns-agent`（linux） | `cargo build -p fns-agent --release --target …` | 用户服务器 / 内部分发 |

每个产物旁保留：git SHA、构建时间、构建机 OS/arch、版本号。
