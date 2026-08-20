# Decisions(决策记录 · ADR)

用 ADR(Architecture Decision Record)记录决策:为什么这样调度、为什么定这种结构、为什么划这条边界。**本目录是全仓决策记录的唯一落点**——功能内决策与框架级决策都记这里,feature 文档只描述设计现状,不留决策记录。

> **这是空模板**——本目录不携带任何具体决策(种子自身的早期设计决策留在 git 历史,不占用下游)。你的项目从 `0001` 开始写自己的。

## 怎么写一条 ADR

- 一个决策 = 一个文件 `NNNN-<slug>.md`,编号从 `0001` 递增;写完在下方索引加一行。
- **一旦记录不改写**:被推翻就新增一条,把旧的状态标成「被 NNNN 取代」,历史留痕。
- 无 frontmatter。格式照抄:

      # NNNN · <一句话决策>

      - 日期:YYYY-MM-DD
      - 状态:生效 | 被 NNNN 取代

      ## 背景
      面对什么问题。

      ## 决策
      定了什么。

      ## 后果
      接受了什么代价。

## 索引

| 编号 | 决策 | 状态 |
|------|------|------|
| [0001](0001-lumio-credentials-local-file.md) | Lumio 凭据本期存 Lumio 自有数据目录的受限权限文件，不引入系统凭据库依赖 | 生效 |
| [0002](0002-sub2api-single-account-source.md) | 以 Sub2API 为唯一账号中心，cchaven-control 降级为纯业务服务 | 生效 |
| [0003](0003-monorepo-three-way-merge.md) | 双仓合并为 codex/ + cchaven/ + web/ 三块并列的 monorepo，subtree 保留历史 | 生效 |
| [0004](0004-cchaven-desktop-union-merge-recovery.md) | CC避风港桌面端以引擎兼容的 P2 为骨架，把 P1 的产品外壳整段移植上去 | 生效 |
| [0005](0005-lumio-first-official-app-install.md) | 首次允许在 Lumio 内下载原样官方桌面应用，镜像优先、客户端集中源常量 | 生效 |
| [0006](0006-official-app-install-destination.md) | 官方应用首次安装允许用户选择安装目录，Windows 便携路线升为一等选项但默认仍走 MSIX | 生效 |
| [0007](0007-bestcodex-apex-portal-coexistence.md) | 产品站占用 bestcodex.app 营销 apex，门户本期内保持独立部署 | 生效 |
| [0008](0008-canonical-account-host-handoff.md) | 规范账号主机是 bestcodex.app；遗留 lumiogame.com 回跳并用 hash 一次性交接会话 | 生效 |
| [0009](0009-web-support-bubble-static-channels.md) | 官网客服气泡初版把社群入口写进前端配置，不接后台 | 生效 |
| [0010](0010-web-support-qq-group-number.md) | 客服气泡的 QQ 入口是群号（可复制），不是加群 URL | 生效 |
| [0011](0011-windows-msix-store-scaffold.md) | Windows 商店包另开 unsigned MSIX 轨，不改 NSIS / ZIP / Tauri bundle | 生效 |
| [0012](0012-claude-balance-subscribe.md) | Claude 包月钱在 Sub2API、订阅权在控制面，不在 Sub2API 做套餐 | 生效 |
| [0013](0013-bestcodex-sync-components-bundling.md) | 同步组件为构建产物不入库，占位由 build.rs 生成，fns-server 源留仓外 pin | 被 0014 取代 |
| [0014](0014-fns-server-in-repo.md) | fns-server 源落仓内独立维护，不再绑定外部 git | 生效 |
| [0015](0015-claude-workspace-scheme-d.md) | Claude 工作台采用方案 D：三栏 + 分阶段，远端 CLI 走官方安装器 | 生效 |
