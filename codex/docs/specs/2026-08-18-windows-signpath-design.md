# Lumio Codex Windows SignPath 签名

## 背景

内部通道此前只出 `-internal-unsigned` Windows 包。2023-06 起新发的 Authenticode 私钥不可导出，不能再把 PFX 放进 GitHub Secrets。本设计用 SignPath Foundation 免费云签，先签 Windows；公开闸门、macOS 公证、`latest.json` 不做。

## 决策

- 第一选择：SignPath Foundation。证书发行者是 SignPath Foundation。申请被拒或不能接受该发行者时，只换 CI 提交步骤（Azure Artifact Signing 或 SSL.com eSigner）。
- `pull_request` 始终未签名。`publish` / `v*` / `workflow_dispatch` 且存在 `SIGNPATH_API_TOKEN` 时，Windows 只出已签名文件名。
- 指针仍是 `latest-internal.json`，`channel` 为 `internal`，每个 asset 带 `signed`。不写 `latest.json`。
- 先签两个 payload exe，再打 NSIS，再签 setup。不签安装时生成的 `uninstall.exe`。

## 非目标

打开 Public release gate、macOS Developer ID、签 CCHaven、把 PFX 入库。
