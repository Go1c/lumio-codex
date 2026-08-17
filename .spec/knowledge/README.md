---
name: knowledge
description: 项目知识库导航——查"某事怎么做"(standards)或"某功能怎么设计的"(features)时,从这里找到对应 .md
metadata:
  type: index
---

# Knowledge(项目知识库 · 导航)

本文件是 `knowledge/` 下所有 .md 的导航 meta:一行描述 + 路径,按需下钻。

> **导航行与各文档 frontmatter `description` 同一句话口径,只写「是什么 + 何时查」。** 交付历史在 git,不进文档;长度 / status 枚举 / 登记覆盖 / 链接可达由 `node .spec/tools/spec-lint.mjs` 机械校验。

## standards/(开发规范 · 要遵守的「怎么做」)

| 文档 | 一句话 |
|------|--------|
| [`standards/workflow.md`](standards/workflow.md) | 开发工作流:分支/提交/合并·PR 与知识同步义务——动手改代码、开 PR 前查 |
| [`standards/code-style.md`](standards/code-style.md) | 代码与文档风格:语言约定、命名、注释原则、生成物纪律——写代码/建文档时查 |
| [`standards/testing.md`](standards/testing.md) | 测试与验收:测试分层政策、TDD 时机、验收 DoD 与验证证据——实现功能/修 bug 时查 |
| [`standards/dispatch.md`](standards/dispatch.md) | 派活模板:worker 派遣与 reviewer 触发的 prompt 骨架——主 loop 扇出任务或触发审查时查 |
| [`standards/ops-release.md`](standards/ops-release.md) | 部署/打包/发版/后台维护入口——准备上线推送、打安装包、发 Release 或改运维流程时查 |

## features/(功能设计与记录 · 供了解)

| 文档 | 一句话 |
|------|--------|
| [`features/_TEMPLATE.md`](features/_TEMPLATE.md) | 新功能文档模板——新增功能记录时照此建,放对 领域 / 模块 |
| [`features/lumio-account-and-home.md`](features/lumio-account-and-home.md) | Lumio 桌面账户与首页：注册/登录/2FA、provisioning、在线离线首页、修复与设置——改账户壳或启动编排时查 |
| [`features/lumio-unified-portal-and-identity.md`](features/lumio-unified-portal-and-identity.md) | 统一门户与统一身份：三站分工、Sub2API 唯一账号源、跨子域会话、控制面令牌校验、充值落点——改账号面或站点时查 |
| [`features/lumio-web-support-bubble.md`](features/lumio-web-support-bubble.md) | 官网右下角客服气泡：QQ 群号可复制、飞书群外链，三站 SiteShell 共用——改社群入口或气泡交互时查 |

## lessons(经验教训 · 复发问题暂存区)

| 文档 | 一句话 |
|------|--------|
| [`lessons.md`](lessons.md) | 经验教训:reviewer 反复退回的同类问题与 Agent 常犯坑——开工前与复盘沉淀时查 |

---

新增 / 修改 / 维护知识文档(放哪、frontmatter、同步本导航)→ 用 `spec-steward` 技能;决策记录(唯一落点)→ [`../decisions/`](../decisions/README.md)。
