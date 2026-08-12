# 04 · 后台服务（api.lumio.games / Sub2API）

桌面客户端**不托管**账户与计费逻辑。生产 API 写死为：

```text
https://api.lumio.games/
```

见 `crates/codex-plus-core/src/lumio/product.rs` → `API_BASE_URL`。

后台实现应是你们部署的 **Sub2API**（上游文档见 Sub2API 仓库 README / `deploy/`）。本仓只约定**桌面依赖的契约与验收**。

## 1. 桌面端实际调用的能力

| 能力 | 典型路径 / 行为 | 客户端位置 |
|------|-----------------|------------|
| 公开设置 | `GET` 公开 settings（注册开关、邮箱后缀、协议、默认模型等） | `lumio::api` → `lumio_public_settings` |
| 验证码 / 注册 / 登录 / 2FA / 刷新令牌 | Sub2API auth 系列 | `lumio::api` + `session` |
| 用户资料（余额等） | `auth/me` 一类 | provisioning / 刷新 |
| 桌面 Key | `GET/POST /keys`，保留名 `Lumio Codex Desktop`，创建带 `Idempotency-Key` | `lumio::account` |
| 模型目录 | 使用桌面 Key 拉取 | provisioning `sync-models` |
| 支付 | **当前**：浏览器打开官网 `/payment`，**不**强制 handoff API | `HomeView` + `SITE_BASE_URL` |

架构规格中的 `GET /api/v1/desktop/config`、`POST .../payment-handoffs` 若后台尚未部署，客户端已用公开设置 + 打开网站降级；后续补齐时需同步改本仓解析与 [03](./03-release.md) 说明。

## 2. 部署后台（操作落点）

在 **Sub2API 部署仓库 / 服务器**上执行（勿把生产 `.env` 写进 lumio-codex）：

1. 按 Sub2API 官方 Docker 部署文档准备 `docker-compose` 与 `.env`  
2. 将公网 HTTPS 反代到服务（证书、HTTP/2、超时适合流式）  
3. DNS：`api.lumio.games` → 该入口  
4. 配置：注册开关、邮箱验证、邀请码、支付渠道、默认模型等  
5. 确认 CORS / Cookie 策略与支付前端一致（若支付在 API 同源前端）

安装类命令以 Sub2API 文档为准，例如其 README 中的 `deploy/install.sh` / `docker-deploy.sh`（版本与镜像名以你们锁定的 fork 为准）。

## 3. 与官网、桌面的衔接

```text
api.lumio.games     ← App 的 JSON API、（可选）账户网页 /support /reset-password
lumio.games         ← 营销站；/payment 应能到达真实充值体验
lumio.games/payment ← App「充值」按钮目标
```

登录页「联系支持 / 重置密码」使用 **`apiBaseUrl`**（即 API 源），不是官网源。请保证：

- `https://api.lumio.games/support`（或你们实际路径）可用，或改客户端路径前先改产品文案  
- `https://api.lumio.games/reset-password` 在「密码重置开放」时可用  

## 4. 上线前健康检查

```bash
# 公开面应快速 200（具体 path 以你们 Sub2API 为准）
curl -sS -o /dev/null -w "%{http_code}\n" https://api.lumio.games/

# 从一台干净机器跑桌面冒烟：注册/登录/拉余额/创建桌面 Key
```

检查表：

- [ ] TLS 证书有效  
- [ ] 注册 / 登录 / 2FA / 刷新令牌在 App 内跑通  
- [ ] 桌面 Key 查找或创建成功；官方 Codex 能走 Lumio 路由  
- [ ] 余额刷新有真实数字（非长期假 0.00）  
- [ ] 支付页从 App 打开后可完成充值（后台侧）  
- [ ] 限流与风控错误能映射到客户端稳定码（见 `lumio/errors`）  

## 5. 后台变更时如何维护本仓文档

后台改了路径、开关或错误 reason 时：

1. 更新 Sub2API 自己的 changelog / ops 文档  
2. 若影响桌面：改本仓 `lumio::api` / `errors` 映射，并补测试  
3. 在本文件 §1 表格改「典型路径」  
4. 发桌面版时在 Release 说明写「需后台 ≥ x.y」  

禁止只改服务器、不改客户端映射却宣称「兼容」。

## 6. 秘密

以下只存在于密钥库 / 主机环境 / CI secrets：

- 数据库 URL、JWT 密钥、支付商密钥、SMTP、对象存储  
- 管理员初始密码（首次部署日志查看后立即轮换）  

本仓 PR 与 Issue 中禁止粘贴上述内容。  
