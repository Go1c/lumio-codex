# 0005 · 首次允许在 Lumio 内下载原样官方桌面应用，镜像优先、客户端集中源常量

- 日期：2026-08-15
- 状态：生效

## 背景

架构规格原先把「不下载官方应用」写成非目标。结果是：本机从没装过官方 Codex / ChatGPT 桌面应用时，首页「启动 Codex」禁用，注释把人赶到设置里手选路径。没装过的人没有路径可选，形成死胡同。

本题有意打开「本机缺失时首次自动安装原样官方桌面应用」这一缺口，不是改成自研 Codex，也不是把官方包打进 Lumio 安装包。

## 决策

1. 打开首次自动安装：用户点首页主按钮后，在 Lumio 内走检测 → 计划 → 下载 → 校验 → 安装 → 再检测 → 无注入启动。仍禁止把官方 DMG/MSIX 捆绑进 Lumio 安装包、禁止修改官方包、禁止代管装好后的版本更新。
2. 默认下载源是 `codex-app-mirror`（清单 + 分架构 payload）。镜像失败再用官方备用：macOS 为 oaistatic DMG，Windows 为 DisplayCatalog + FE3 直拉产品 `9PLM9XGG6VKS`，不打开 Microsoft Store UI。
3. Sub2API `GET /api/v1/desktop/config` **本期不转发**清单 URL / sha256 / 分架构地址。这些只读字段集中在客户端 `official_app_install/sources.rs`。这是已知缺口，等服务端加安全只读字段再迁。

## 后果

- 换镜像 URL 或清单形状必须发版客户端，不能只改服务端配置。
- 镜像被攻仍不能装假包：还必须过 OpenAI 原签名（Windows Authenticode Marketplace 钉选；macOS Team ID `2DC432GLL2` + Gatekeeper）。
- 装好后的更新仍交给官方（Win 商店 / 应用内，Mac Sparkle）；Lumio 只负责第一次装上，以及「装了但检测不到」时再装一次。
