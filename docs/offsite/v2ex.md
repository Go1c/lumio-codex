# V2EX 草稿

**节点建议：** `programmer` 或 `macos`  
**标题：** 做了个启动器：零配置开官方 Codex，另一个 Tab 把 Claude Code 跑在自己的服务器上

---

自己用 Codex 的时候最烦填 Base URL / API Key；用 Claude Code 又担心本机出口 IP 和其他软件混在一起，看起来像异常环境。

BestCodex 是一个桌面启动器（macOS / Windows），一个窗口两个 Tab：

- **Codex**：App 内登录一次，只写它自己管的连接配置（先备份，可恢复），然后启动的是**官方 Codex 应用本身**。不捆绑、不改官方应用。
- **Claude**：把官方 Claude Code 跑在你自己的服务器上——独立环境、固定 IP、持久会话、文件双向同步。机密文件默认不同步，冲突并排对比，不静默覆盖。

说在前面：**没有任何方案能保证不被封。** 服务条款和风控是 Anthropic 的，第三方只能降低环境侧的风险。

```
curl -fsSL https://bestcodex.app/install.sh | sh
```

官网 https://bestcodex.app  
当前是未签名内测包，macOS 可能报「已损坏」，脚本会清隔离标记。桌面端是 Codex++ 的 AGPL fork。独立项目，和 OpenAI / Anthropic 无从属关系，也和网上同名的 API 中转无关。
