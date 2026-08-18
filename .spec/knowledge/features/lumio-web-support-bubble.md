---
name: lumio-web-support-bubble
description: 官网右下角客服气泡：QQ 群号可复制、飞书群外链，门户与 BestCodex 的 SiteShell 共用——改社群入口或气泡交互时查
metadata:
  type: doc
  status: 已交付
---

# 官网客服气泡

门户与 BestCodex 产品站右下角一枚聊天气泡，打开后只提供 QQ 群号（点击复制）与飞书群外链。没有在线对话、没有工单表单。

## 背景 / 目标

- 用户在官网要能立刻找到人，不必先翻页脚或账户菜单。
- 初版只要社群入口，交互对齐 Workflow 的 launcher（圆钮 + 三点 + 展开面板），能力面刻意收窄。

## 设计

- **交互面**：固定右下角；点击展开 / 再点或 Esc / 点面板外关闭。QQ 是群号卡片，点击复制；飞书是外链卡片，`target=_blank`。某条值为空则不渲染那张卡；两条都空则整枚气泡不出现。
- **实现面**：`@lumio/ui` 的 `SupportBubble` 挂在 `SiteShell` 末尾，门户与 BestCodex 产品站自动带上。入口只在 `supportChannels()`（`packages/ui/src/config.ts`）读取，环境变量 `VITE_SUPPORT_QQ_NUMBER` / `VITE_SUPPORT_FEISHU_URL` 可覆盖；写 `off` 可关掉一条通道。
- **现状**：QQ 群号默认 `1073671738`；飞书群默认
  `https://applink.feishu.cn/client/chat/chatter/add_by_link?link_token=802t132e-f554-4ec2-9b18-5f83276fcb9f`。

## 待解决

- 飞书加群 token 若过期，改 `config.ts` 默认值或覆盖环境变量后重新构建。

## 相关

- [ADR-0009 客服气泡初版写进前端配置](../../decisions/0009-web-support-bubble-static-channels.md)
- [ADR-0010 QQ 入口是群号不是 URL](../../decisions/0010-web-support-qq-group-number.md)
- 统一官网开发说明：[`web/README.md`](../../../web/README.md)
- Workflow 参考实现：`LumioGameWorkFlow/apps/web/src/shared/support/SupportBubble.tsx`（本仓不引用其代码）
