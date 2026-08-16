# 05 · 日常维护与文档同步

## 1. 角色与节奏

| 频率 | 事项 |
|------|------|
| 每次合并到 `publish` | CI 内部包是否绿；失败当日修 |
| 每个桌面版本 | 走 [03-release.md](./03-release.md) 检查清单 |
| 每周 | 抽测生产 API + 官网 + 一台上安装的内测包冷启动 |
| 每月 | 证书到期、磁盘、Sub2API 备份、GitHub artifact 是否还够用 |
| 签名证书就绪时 | 单独立项打开 Public release gate，改 [03](./03-release.md) §4 |

## 2. 三条生产线怎么养

### 2.1 桌面客户端（本仓）

- 功能开发在 `publish`（或短命 feature 分支 → PR → `publish`）  
- 验证命令见 [01](./01-local-build.md) §3  
- 用户可见文案变更要对照 UX 规格与 BestCodex 品牌口径；禁词见 `shell-copy.test.ts`  
- 发版才打 tag / Release  

### 2.2 官网（用户可见 `https://bestcodex.app`）

- 新站在 `web/`；旧 `site/` 只做过渡，勿再加功能  
- 部署见 [02](./02-website-deploy.md) 与根 `docs/ops/`  
- `https://api.lumio.games/purchase` 故障当作 **P0**（充值入口）  
- 帮助页 `https://bestcodex.app/help` 不可用当作 **P1**  

### 2.3 后台（Sub2API）

- 变更在后台仓完成；本仓只跟契约  
- 任何破坏性 API 变更必须：**先兼容桌面 → 再发后台 → 再发依赖新契约的桌面**  
- 文档入口 [04](./04-backend.md)  

## 3. 文档维护义务（「全部完成」的定义）

改动合并前，按下表更新，避免运维手册漂移：

| 你改了什么 | 必须更新 |
|------------|----------|
| 打包脚本 / CI workflow / 产物名 | [01](./01-local-build.md)、必要时根 README「内部测试包」 |
| `site/` 或域名 / Pages | [02](./02-website-deploy.md) |
| 版本策略 / Release / 更新提醒 / 签名 | [03](./03-release.md) |
| API 基址、支付方式、后台依赖 | [04](./04-backend.md)、`product.rs` 注释 |
| 分支策略、发版节奏 | 本文件 + [ops/README.md](./README.md) |
| 产品行为 / 文案基线 | `docs/specs/…`、`.spec/knowledge/features/…` |
| Agent 规则 / 技能 | `.spec/` + `node .spec/tools/spec-lint.mjs` |

知识库沉淀用 `spec-steward` 技能；**运维手册以本目录为唯一入口**，不要在聊天里另起一份互相打架的流程。

## 4. 监控与告警（最低集）

| 对象 | 怎么盯 |
|------|--------|
| `api.lumio.games` | 外部 HTTP 探测 + Sub2API 日志 |
| `bestcodex.app`（含 `/help`） | 新站 / CDN 可用性；CORS / DNS 仍待运维 |
| `lumio.games` | 旧站 301 到 `https://bestcodex.app` |
| GitHub Actions | `publish` 推送后内部构建失败邮件/通知 |
| 支付 | 支付商后台成功率；`https://api.lumio.games/purchase` 拨测 |
| 客户端崩溃 | 待遥测正式接入前：内测群收集 + 诊断日志导出 |

## 5. 事故时优先顺序

1. 用户是否还能 **离线启动官方 Codex**（本机配置健康时）  
2. 认证 / 刷新令牌是否大面积失败 → 查 API  
3. 充值是否 404 → 查 `https://api.lumio.games/purchase` 与后台支付  
4. 错误发版 → 按 [03](./03-release.md) §6 回滚  
5. 事后：在 `.spec/knowledge/lessons.md` 记复发项（同类第二次才收录）  

## 6. 仓库内文档地图

```text
docs/ops/                         ← 你在这里（部署 / 打包 / 发版 / 后台 / 维护）
docs/specs/                       ← 产品与交互定稿
docs/plans/                       ← 历史实现计划（不是运维手册）
.spec/                            ← Agent 规则、知识、ADR
README.md                         ← 用户向总览 + 链到 ops
site/                             ← 旧官网源码（用户可见站点已是 bestcodex.app）
.github/workflows/                ← CI 打包与公开发布闸门
```

根 README「开发状态」过时时应与本目录同步修订，避免对外口径与运维事实不一致。
