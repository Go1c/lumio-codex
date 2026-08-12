# Lumio Codex 品牌客户端设计

## 背景

本仓库是 Codex++ 的 AGPL-3.0 Fork。目标不是继续扩展 Codex++ 的增强面板，而是交付一个面向 LumioAPI 用户的轻量桌面入口：用户在 Lumio Codex 中完成协议确认、邮箱注册或登录、API Key 初始化和官方 Codex 配置接管，随后继续使用官方 Codex 原生界面。

服务端继续使用独立仓库 `Go1c/sub2api`。客户端仓库负责桌面体验、系统凭据存储、官方应用检测、Codex 配置接管和更新；Sub2API 负责准入、账户、额度、套餐、模型目录、远程桌面配置和支付登录交接。

首个里程碑只生成明确标记为内部测试的未签名制品。公开发布必须同时满足 Apple Developer ID 签名与公证、Windows 代码签名、更新清单签名、S3 同步和回滚演练门槛。

## 目标

1. macOS arm64、macOS x64 和 Windows x64 使用同一产品流程。
2. 用户无需输入 Provider、Base URL、协议或 API Key。
3. 动态遵守 Sub2API 的注册开关、邮箱验证、邮箱后缀、协议和区域声明。
4. 复用或创建账号级唯一的 `Lumio Codex Desktop` API Key。
5. 只接管 Codex 配置中由 Lumio 拥有的段，并可恢复接管前快照。
6. 服务暂时不可用时，已登录且本地已有有效配置的用户仍可启动官方 Codex。
7. 支付交接不在 URL 中暴露 JWT、API Key 或刷新令牌。
8. 更新制品在下载和安装前校验版本、平台、架构、长度、SHA-256 和清单签名。
9. 遥测默认关闭，开启后仍不采集用户内容或身份凭据。

## 非目标

- 不捆绑、下载或修改官方 Codex/ChatGPT 应用二进制。
- 不增加第三方 OAuth、邀请码、Turnstile、设备管理或内置支付 UI。
- 不向用户恢复 Codex++ 的 Provider、多供应商、脚本、Stepwise、Goals、MCP、Skill、Plugin、注入或会话增强入口。
- 不建立新的用户库，不在客户端写死试用额度。
- 不在首个里程碑执行公开发布、生产部署或证书申请。

## 选定架构

采用“独立 Lumio 外壳 + 复用底层能力”的渐进式 Fork：

- 新建独立的 Lumio React 应用入口、状态机、API 客户端和 Tauri 命令白名单。
- 复用经过测试的官方应用检测、跨平台启动、Codex 路径解析和配置解析能力。
- 旧 Codex++ 页面及增强模块不从 Lumio 路由或菜单暴露，增强总开关固定关闭；首版不大规模删除底层代码，以降低上游同步和平台回归风险。
- 新功能放入以 `lumio_` 或 `lumio-` 命名的隔离模块；不继续扩张现有超大 `App.tsx`。
- Sub2API 以增量接口支持桌面配置和支付交接，不复制账户与计费逻辑。

相比原地删除旧模块，该方案更容易审查安全边界并保留上游同步能力；相比另建第二个 Tauri workspace，它避免复制安装、检测、启动和打包适配。

## 系统边界

### 客户端仓库

客户端包含六个边界清晰的单元：

1. `lumio_api`：只处理 Sub2API HTTP 契约、超时、刷新和错误归一化。
2. `lumio_credentials`：只读写操作系统凭据库，不输出明文。
3. `lumio_account`：登录状态机、2FA、账户刷新、Key 初始化和离线资格判断。
4. `lumio_codex`：官方应用检测、手选路径、快照、Lumio Provider 合并、恢复和启动。
5. `lumio_update`：双源清单选择、签名与制品校验、内部/正式渠道门槛。
6. `LumioApp`：仅渲染产品允许的首页、注册/登录和设置，不持有秘密处理逻辑。

Tauri 只向 Lumio 页面注册所需命令。旧命令即使仍在源码中，也不得进入 Lumio 的 `invoke_handler` 白名单。

### Sub2API 仓库

服务端继续复用现有接口：

- `GET /api/v1/settings/public`
- `POST /api/v1/auth/send-verify-code`
- `POST /api/v1/auth/register`
- `POST /api/v1/auth/login`
- `POST /api/v1/auth/login/2fa`
- `POST /api/v1/auth/refresh`
- `POST /api/v1/auth/logout`
- `GET /api/v1/auth/me`
- `GET/POST /api/v1/keys`
- `GET /api/v1/groups/available`
- `GET /api/v1/groups/rates`
- `GET /api/v1/subscriptions/summary`
- `GET /v1/models`（API Key 鉴权的 Codex 模型目录）

新增两个服务端能力：

- `GET /api/v1/desktop/config`：公开、只读、可缓存的桌面配置。
- `POST /api/v1/desktop/payment-handoffs` 与一次性消费路由：创建短时支付交接并在浏览器建立 HttpOnly 会话。

## 客户端状态机

应用启动按以下阶段推进：

1. `bootstrapping`：加载非敏感设置、凭据存在性、缓存桌面配置和官方应用路径。
2. `checking-service`：并行获取公开设置与桌面配置；失败时保留缓存并标记服务不可用。
3. `signed-out`：展示协议和注册/登录入口。
4. `authenticating`：邮箱验证码、注册、密码登录或 2FA。
5. `provisioning`：刷新用户、选择可用分组、确保桌面 Key、读取模型目录、写入 Lumio 配置。
6. `ready-online`：展示实时余额、套餐、连接状态和默认模型。
7. `ready-offline`：仅当安全存储中有凭据、存在已验证的本地接管记录、官方应用可用且上次配置未被撤销时进入；允许启动和导出日志，禁用注册、刷新和充值。
8. `needs-repair`：凭据、Key、快照或配置不一致；提供安全恢复，不静默覆盖用户配置。

状态转换使用纯函数测试。网络错误、认证错误、配置错误和系统凭据错误采用不同错误码，不以字符串判断业务分支。

## UI 信息架构

### 首页

首页只展示：

- 当前账号（邮箱只在账户区域显示；日志和遥测不包含邮箱）。
- 余额、试用额度的服务端最终结果和套餐摘要。
- LumioAPI 连接状态与缓存时间。
- 服务端默认模型及当前已配置状态。
- “充值”和“启动 Codex”两个主要动作。

### 注册与登录

注册页以 `settings/public` 为唯一动态规则来源，展示：

- 注册是否开放。
- 是否必须邮箱验证及验证码入口。
- 允许的邮箱后缀提示。
- 服务条款、隐私政策、使用政策和区域声明的当前版本与链接/正文。
- 必须显式勾选的协议版本。

客户端只做输入提示和必填校验，不自行判断地理位置；服务端 IP 与账号风控结果是最终裁决。登录支持密码、2FA 和密码重置入口，不显示 OAuth。

### 设置

设置仅包含：

- 开机启动。
- 自动更新。
- 官方应用路径与重新检测/手动选择。
- 遥测开关（默认关闭）。
- 日志导出。
- 配置恢复。

Provider、Base URL、Key、协议、多供应商和全部 Codex++ 增强配置不渲染，也不存在可导航入口。

## 凭据与本地数据

### 操作系统凭据库

认证 Token 与 API Key 使用不同记录：

- service：`games.lumio.codex.auth`，account：`sub2api-session`
- service：`games.lumio.codex.api-key`，account：`lumio-codex-desktop`

macOS 使用 Keychain，Windows 使用 Credential Manager。访问令牌与刷新令牌作为一个版本化 JSON 凭据保存；API Key 单独保存。Rust 接口只返回“存在/缺失/失效”状态，只有发起请求或应用配置时在后端进程内短暂取得明文，前端永不接收完整 API Key。

日志、错误对象、崩溃上下文和 Tauri command 返回值统一经过脱敏器。脱敏器覆盖 Bearer、`sk-` 类 Key、JWT、邮箱、URL 查询/片段中的敏感字段和操作系统路径。

### 非敏感数据

应用数据目录使用 Lumio 自有目录，不复用 Codex++ Manager 数据目录。只保存：

- UI 设置和官方应用路径。
- 最近一次成功的公开设置与桌面配置及获取时间。
- 已脱敏账户摘要。
- 配置接管清单、快照位置、快照哈希和本地配置状态。
- 已验证更新清单的非敏感元数据。

缓存文件使用原子写入；macOS 权限为用户可读写，Windows ACL 仅授予当前用户。

### 官方 Codex 兼容副本

操作系统凭据库是 Lumio 的秘密来源。官方 Codex 当前需要其支持的认证配置才能直接请求自定义 Provider，因此接管期间允许把 API Key 写入 Codex 自有认证位置或 Lumio Provider 的受管凭据字段，但必须满足：

- 只写入快照清单声明的 Lumio 所有权字段。
- 文件权限收紧为当前用户可读写。
- 不写入 Lumio 普通设置、日志或 UI 状态。
- 退出账号或恢复配置时删除该副本并恢复接管前内容。
- 写入中断时通过临时文件、落盘同步和原子替换避免半份配置。

这是官方 Codex 兼容边界，不将其描述为操作系统安全存储。若后续官方 Codex 提供跨平台凭据引用机制，可移除该副本而不改变上层账户接口。

## API Key 初始化

固定名称为 `Lumio Codex Desktop`，算法如下：

1. 获取当前用户可用分组，选择服务端允许的默认/首选 Codex 分组。
2. 分页查询同名 Key，优先复用最早创建且仍为 active、未过期、分组仍可用的 Key。
3. 不存在有效 Key时，调用现有 `POST /api/v1/keys` 创建，发送固定请求体和 `Idempotency-Key`。
4. 创建后重新查询并以服务端结果为准，再写入安全存储。
5. 本地 Key 被服务端拒绝时清除安全存储、重新查询；若同名有效 Key存在则复用，否则进入一次恢复创建。

并发保证不能只依赖客户端互斥。Sub2API 对保留名称增加账号级串行化的“查找或创建”语义，但仍经现有 `/keys` 创建入口暴露；普通 Key 名称保持原行为。服务端已有用户级幂等协调器用于重放同一创建请求，账号级保留名称逻辑负责跨设备不同请求的最终唯一性。

用户手动停用的同名 Key不会被静默重新启用；客户端创建新的有效代次，并在 UI 中只称为“已恢复连接”。被删除的 Key不从软删除记录复活。

## 桌面配置接口

`GET /api/v1/desktop/config` 返回标准 envelope，`data` 结构为：

```json
{
  "schema_version": 1,
  "default_model": "gpt-example",
  "payment_path": "/payment",
  "minimum_client_version": "1.0.0",
  "update_notice": {
    "level": "info",
    "title": "",
    "message": ""
  },
  "features": {
    "registration": true,
    "payment_handoff": true,
    "telemetry": false
  }
}
```

约束：

- 接口公开只读，不返回秘密或按用户变化的数据。
- `payment_path` 只能是同站点绝对路径，客户端拒绝跨域 URL。
- `default_model` 必须经过服务端模型配置校验。
- 支持 `ETag`、`Cache-Control`，客户端缓存最近一次成功响应。
- 未命中缓存时使用安全回退：不覆盖现有模型、关闭注册/支付交接/遥测、允许显示服务不可用；不猜测模型名或额度。
- 客户端版本低于最低版本时禁止新的登录与接管，但已登录且本地配置有效时仍可离线启动，并显示强制更新提示。

默认模型和功能开关作为 Sub2API 管理设置保存；后台调整后无需发布客户端。

## 支付交接

### 创建

客户端使用当前 JWT 调用：

```http
POST /api/v1/desktop/payment-handoffs
Authorization: Bearer <access-token>
Idempotency-Key: <random-per-click>
```

服务端生成至少 256 bit 随机秘密，响应只返回一次消费 URL 和过期时间。JWT、刷新令牌和 API Key均不进入 URL。消费 URL只携带随机一次性秘密。

服务端存储：

- 仅保存一次性秘密的 SHA-256 哈希，不保存原文。
- 记录用户 ID、创建时间、过期时间和固定目标 `/payment`。
- TTL 60 秒；同一用户限制未消费交接数量和创建频率。
- Redis/持久层不可用时失败关闭，不签发无法可靠单次消费的令牌。

### 消费

系统浏览器访问消费 URL后，服务端执行原子 get-and-delete：

1. 校验格式、哈希、TTL 和未消费状态。
2. 若浏览器已有另一用户的 Lumio 支付会话，返回冲突页，不静默切换用户。
3. 为绑定用户创建短时网站会话，设置 `Secure; HttpOnly; SameSite=Strict` Cookie。
4. 同时设置随机 CSRF 双提交 Cookie；Cookie 认证的写请求必须携带匹配的 `X-CSRF-Token`。
5. 返回 `Cache-Control: no-store`、`Referrer-Policy: no-referrer`，并以 303 重定向到固定 `/payment`。

支付会话有效期 30 分钟，只允许同源站点 API，不能兑换为刷新令牌。JWT 中包含支付会话 audience；服务端 JWT 中间件可从 HttpOnly Cookie 读取该 audience，并继续执行用户状态、TokenVersion 和会话绑定检查。Bearer 认证保持原有优先级与行为。

Sub2API 前端在没有 localStorage Token 时调用 `/auth/me` 探测 Cookie 会话；成功后只把用户资料放入内存，不把 Cookie 中的 JWT复制到 JavaScript 存储。支付页及其 API 客户端使用 `withCredentials` 和 CSRF Header。会话过期后回登录页。

消费成功、过期、重复使用、格式错误、另一用户冲突和存储故障均使用不包含秘密的稳定错误码。

## Codex 应用检测与启动

复用 Codex++ 已有平台解析逻辑并收窄输出：

- macOS：扫描 `/Applications`、`~/Applications` 中官方 Codex/ChatGPT 应用，解析 bundle 与可执行文件。
- Windows：扫描 Store 包注册信息和常见安装路径，支持 packaged activation。
- 自动检测失败时使用系统文件选择器，验证签名主体/包身份、可执行文件名和平台结构；首个内测阶段至少验证结构与产品标识，并明确显示“未验证签名”状态。
- 不下载、不解包、不捆绑官方应用。

“启动 Codex”前执行：

1. 读取安全存储和本地接管状态。
2. 在线时刷新桌面配置、账户、Key 与模型目录；离线时验证缓存和配置哈希。
3. 必要时合并 Lumio Provider 配置。
4. 启动官方应用，不启用注入、CDP 会话增强、脚本、插件或其他 Codex++ watchdog。
5. 只记录启动阶段和脱敏错误码。

## 配置接管、快照与恢复

### 所有权模型

Lumio 只拥有：

- 根级当前模型指向（仅在应用服务端默认模型时设置）。
- 固定名称的 Lumio model provider 段。
- Lumio 模型目录指针或由服务端目录生成的受管 catalog 文件。
- 官方 Codex 兼容所需的 Lumio API 凭据字段。

其他 Provider、MCP、projects、profiles、用户模型和未知字段均归用户所有，不删除、不重排、不格式化整个文件。

### 首次接管

首次写入前：

1. 解析并验证当前 `config.toml` 与相关认证文件。
2. 将原始字节复制到 Lumio 私有快照目录，记录 SHA-256、权限和是否原本不存在。
3. 使用独占锁避免与另一个 Lumio 实例并发写入。
4. 通过结构化 TOML 编辑只合并 Lumio 字段。
5. 写临时文件、同步、保留原权限并原子替换。
6. 重读并验证目标字段及未知字段未变化。

快照只创建一次；后续更新 Lumio 段前记录当前受管字段基线，发现用户在外部修改同一字段时进入 `needs-repair`，不静默覆盖。

### 恢复与退出

手动恢复或退出账号时：

1. 停止 Lumio 启动的本地任务/服务。
2. 删除操作系统凭据库中的 Token 与 API Key。
3. 在锁内恢复接管前快照；若非 Lumio 字段在接管后发生变化，采用三方合并，仅撤销 Lumio 所有字段。
4. 删除受管 catalog 与兼容凭据副本。
5. 重读验证后清除接管记录。

恢复失败时保留快照和诊断信息，界面显示可重试状态，不删除最后可恢复副本。

## 离线行为

离线启动资格必须同时满足：

- 用户此前成功登录且安全存储凭据存在。
- 本地存在最近成功桌面配置和模型目录。
- 固定 API Key 的本地记录未被标记撤销。
- Codex 受管配置与记录哈希一致。
- 官方应用路径仍然有效。

离线模式不承诺服务端请求一定成功，只允许用户启动已配置的官方 Codex。注册、验证码、登录、账户刷新、Key 恢复和充值按钮显示服务不可用。网络恢复后自动重新验证账户与 Key；验证失败时不继续声称连接有效。

## 遥测与日志

遥测默认关闭，设置必须由用户主动开启。允许字段只有：

- 客户端版本。
- 操作系统和架构。
- 启动阶段枚举。
- 脱敏错误码。

禁止字段包括邮箱、用户 ID、Token、Key、提示词、代码、请求/响应内容、文件路径、项目名、模型输入和 URL 查询参数。发送前使用字段白名单序列化，不接受任意 JSON 上报。

本地诊断日志同样不记录秘密；导出前执行第二次脱敏扫描并附带用户可见的字段说明。

## 品牌与许可

品牌替换范围：

- 产品名 `Lumio Codex`。
- bundle/package identifier `games.lumio.codex`。
- 可执行文件、窗口标题、安装包、开始菜单项、数据目录、更新 User-Agent 和错误前缀。
- README、截图、下载链接和更新渠道指向 `Go1c/lumio-codex`。
- 使用独立 Lumio 图标源文件生成 macOS/Windows 所需尺寸，生成物随源提交。

必须保留：

- `LICENSE` 中 AGPL-3.0-only。
- `THIRD_PARTY_NOTICES.md`。
- 对 `BigPizzaV3/CodexPlusPlus` 的 Fork 与上游同步说明。
- OpenAI/Codex/ChatGPT 商标不归本项目所有且官方应用不随包分发的声明。

## 更新与发布

### 清单

GitHub Release 附带版本化 `latest.json`、`latest.json.sig` 和校验文件；CI 把完全相同的字节同步到 S3。清单包含：

- schema version、产品、channel、version、tag、发布时间和最低支持版本。
- 每个制品的平台、架构、类型、文件名、长度、SHA-256、GitHub URL 和 S3 URL。
- release notes 摘要。

客户端内置清单签名公钥。正式渠道必须通过签名验证；私钥只由 CI 环境注入。S3 优先获取，GitHub 回退；若两个来源均可用但版本或制品元数据不一致，则拒绝更新并显示稳定错误码。

下载后先比对长度和 SHA-256，再调用平台安装器。校验失败删除单个下载文件并拒绝启动。更新目录与账户数据目录分离，安装器不得覆盖安全存储、快照或用户非 Lumio 配置。

### 制品

首版构建四类制品：

1. Windows x64 安装器。
2. Windows x64 便携 ZIP。
3. macOS arm64 DMG。
4. macOS x64 DMG。

签名环境未配置时，CI 只能生成带 `internal-unsigned` channel/文件名标记的内测制品，不更新公开 `latest.json`，客户端也不自动安装它们。

### 正式发布门槛

- Apple Developer ID 签名和 notarization/stapling 验证通过。
- Windows Authenticode 签名验证通过。
- 更新清单签名私钥、S3 HTTPS 基址与凭据由 CI 注入。
- GitHub Release 中版本、Tag、清单、长度和 SHA-256 一致。
- S3 同步后逐项回读校验。
- 在隔离环境演练更新失败与版本回滚，确认账户安全存储和配置快照不变。

## 错误处理

稳定错误域：

- `AUTH_*`：注册、登录、2FA、刷新和账号状态。
- `KEY_*`：查找、创建、撤销、系统安全存储。
- `SERVICE_*`：公开设置、桌面配置和账户接口不可用。
- `CODEX_*`：检测、路径、快照、合并、恢复和启动。
- `PAYMENT_HANDOFF_*`：创建、过期、消费、冲突和会话。
- `UPDATE_*`：来源不一致、签名、长度、哈希、平台和安装。

UI 显示可操作的本地化文案；日志和遥测只记录错误码及允许的阶段字段。服务端原始响应先归一化，不能直接把可能含敏感信息的 body 写日志。

## 测试策略

### 客户端单元与集成

- 注册规则：邮箱后缀、验证码、协议版本、注册关闭、重复提交和服务端风控错误映射。
- 登录：密码、2FA、刷新失败、账号禁用、密码重置入口和多设备状态。
- Key：已有复用、首次创建、跨请求并发唯一、撤销恢复和凭据库失败。
- 状态机：在线、离线、最低版本和 repair 分支。
- Codex：macOS/Windows 检测候选、手动路径验证、快照、字段级合并、外部修改冲突、三方恢复和启动参数。
- 安全：所有 Tauri 返回对象与日志脱敏、前端拿不到 Key、遥测默认关闭和白名单序列化。
- 更新：S3/GitHub 选择、元数据不一致、签名失败、长度/哈希失败、平台架构选择和内部渠道禁止自动安装。

### Sub2API

- 桌面配置默认值、管理员配置、ETag 和公开访问。
- 固定名称 Key 的并发查找或创建只产生一个有效 Key；停用/删除后的恢复语义。
- 支付交接创建鉴权、频率限制、随机性、只存哈希、原子单次消费、过期、重复、另一用户冲突和固定重定向。
- Cookie 属性、支付 audience、CSRF、TokenVersion、账号禁用和登出清理。
- 现有 Bearer 登录、支付和外部 auth handoff 回归不变。

### 构建与端到端

- Rust：格式、workspace 测试及相关平台条件编译测试。
- React：TypeScript check、Node tests、Vite build。
- Go：单元测试、`go vet -tags integration ./...` 及相关集成测试。
- Vue：typecheck、单元测试和 production build。
- CI：四类制品、清单一致性、校验失败拒绝、内部/正式门槛。
- 干净 macOS 与 Windows 机器分别完成安装、协议确认、注册、自动建 Key、看到试用余额、启动官方 Codex并发出请求，全程不手填 Base URL 或 API Key。

签名、公证、Windows 实机和生产支付依赖外部凭据/环境；在条件具备前保持为明确的发布门槛，不能用本地单元测试替代其验收证据。

## 实施顺序

1. 品牌与 Lumio 外壳、契约类型、状态机和命令白名单。
2. Sub2API 桌面配置与保留名称 Key 并发语义。
3. 客户端认证、安全存储、账户与 Key provisioning。
4. Codex 检测、快照、字段级合并、恢复与离线启动。
5. 支付一次性交接、Cookie/CSRF 和网站支付页会话。
6. 双源更新、清单签名、四类内测构建与发布门槛。
7. 双仓库回归、安全审计、知识沉淀和双平台端到端验收。

每一步先写失败测试，再实现最小生产代码；服务端改动先进入 Sub2API `dev` 派生的功能分支，客户端改动进入本仓库 `publish` 开发分支。两仓库均不在本任务中推送或公开发布。
