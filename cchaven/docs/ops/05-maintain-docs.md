# 05 · 运维与后台文档维护

文档和代码一样是交付物。发版可以缺功能说明，**不能**缺「怎么部署/怎么回滚」或
「后台接口契约已变但文档仍写旧字段」。

---

## 1. 文档地图（谁维护什么）

| 文档 | 权威程度 | 何时必须改 | 负责人习惯 |
| --- | --- | --- | --- |
| [`docs/ops/*`](./README.md) | 部署/发版真源 | 拓扑、环境变量、发版步骤变化 | 改基础设施的人同一 PR 改 |
| [`services/cchaven-control/README.md`](../../services/cchaven-control/README.md) | 控制面开发入口 | 本地启动、密钥、管理员创建变化 | 改 control 的人 |
| [`services/cchaven-control/docs/m1-spec.md`](../../services/cchaven-control/docs/m1-spec.md) | API/表结构清单 | 增删路由或表 | 改 API 的人 |
| [`services/cchaven-control/.env.example`](../../services/cchaven-control/.env.example) | 环境变量契约 | 新增/改名配置项 | 改 `internal/config` 的人 |
| [`apps/admin/README.md`](../../apps/admin/README.md) | 后台前端契约与 mock | 管理 API 字段、权限、页面流 | 改 admin 或 admin API 的人 |
| [`apps/web/README.md`](../../apps/web/README.md) | 官网契约与 mock | 用户 API、邀请/授权流 | 改 web 的人 |
| [`apps/desktop/README.md`](../../apps/desktop/README.md) | 桌面开发与同步 | 环境变量、打包、同步缺口 | 改 desktop 的人 |
| 各 `docs/spec-gaps.md` | 规范矛盾与未接通 | 缺口闭合或新增 | 发现/闭合缺口的人 |
| [`docs/design/interaction-design.md`](../design/interaction-design.md) | **产品规范，只读** | 不在工程 PR 里改；走产品流程 | 产品/设计 |

`design/prototype/` 是 UI 真源，同样不要在运维文档里复制大段 UI 说明。

---

## 2. 后台（admin）文档维护清单

每次改到管理 API 或后台前端，PR 里自检：

1. **`apps/admin/README.md` 的接口表**是否仍与
   `services/cchaven-control/internal/api/handler_admin.go` 一致？  
2. **权限矩阵**（support 只读 / owner·ops 可写 / 明文邮箱仅详情）是否仍正确？  
3. **MSW mock**（`apps/admin/src/mocks/handlers.ts`）是否同步？  
   mock 是后台开发的日常真源，落后会直接造成错误实现。  
4. **错误码与 6.2 文案**：前端展示 `message`、分支走 `code`；文案改动只在
   `internal/i18n`，admin README 不复制长文案。  
5. **用户 ID 契约**：接口路径用数字 `user_id`，展示号 `U-…` 仅 UI——文档勿写反。  
6. 若影响部署（新环境变量、新反代路径），**同步改 `docs/ops/02-deploy-production.md`**。

建议在 admin PR 模板中固定一项：`[ ] admin README / ops 文档已更新或 N/A`。

---

## 3. 控制面文档维护清单

改 `services/cchaven-control` 时：

1. 新环境变量 → `.env.example` + `docs/ops/02-deploy-production.md` + config 告警文案  
2. 新迁移 → `m1-spec.md` 表清单；发版说明写清是否可回滚  
3. 新公开/管理路由 → `m1-spec.md`；若 web/admin 调用，推动对应前端 README/mock  
4. CORS / cookie / 可信来源语义变化 → **优先改 ops 架构与部署文**，再改组件 README  
5. ADR（如 `docs/adr-0001-*.md`）只追加，不改写历史结论；新决策新开 ADR  

---

## 4. 运维文档（本目录）维护规则

1. **单一入口**：对外说「看 `docs/ops/README.md`」，不要在飞书/群聊另起一份长期真源。  
2. **与代码同 PR**：改部署方式的代码/脚本，必须带文档 diff。  
3. **写事实，不写愿望**：未自动化的流程标明「人工」；缺口链到 `spec-gaps.md`。  
4. **命令可复制**：所有 bash 块在干净环境可跑；路径相对仓库根或写明 `cd`。  
5. **密钥示例用占位符**：文档里永远不要出现真实密码、JWT、SMTP。  
6. **发版后复查**：每次生产推送后 24h 内，确认 ops 文档没有「已过时步骤」（例如旧域名）。  

---

## 5. 推荐的季度文档巡检

每季度或每个大版本后：

- [ ] 按 `02-deploy-production.md` 在 staging 空跑一遍「首次部署」  
- [ ] 核对 `.env.example` 与 `config.go` 字段一致  
- [ ] 抽查 admin README 接口表与 OpenAPI/路由注册一致  
- [ ] 确认 `04-release.md` 里「尚未自动化」列表是否有已完成项可勾掉  
- [ ] 归档过期的口头流程到文档或删除  

---

## 6. 沟通话术（避免双真源）

| 场景 | 正确做法 |
| --- | --- |
| 新人问怎么上线 | 丢 `docs/ops/README.md` 链接 |
| 群里讨论改了反代 | 结论写回 `02-deploy-production.md` 再执行 |
| 产品改了后台交互 | 改规范/原型；工程改代码 + admin README；不改 ops 除非部署变 |
| 临时 hotfix 步骤 | 先写入发版说明，稳定后合并进 `04-release.md` hotfix 节 |
