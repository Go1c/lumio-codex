# Reddit 帖（r/ClaudeAI 或 r/LocalLLaMA）

**Suggested title:** Running official Claude Code on my own server (and a zero-config Codex launcher)

**不要**发在 r/ChatGPT 的“工具广告”区。先回两三个别人的帖，再发自己的。

---

I got tired of two things:

- filling in a base URL / API key just to use official Codex
- Claude Code looking “anomalous” because the session shares my laptop’s IP and everything else I do on that machine

BestCodex is a small desktop launcher (Mac / Windows) with two tabs:

- **Codex tab:** sign in once, it writes only the connection config it manages (backup first, restorable). Then it launches the official Codex app. Not a fork of the official app.
- **Claude tab:** official Claude Code on *your* server — dedicated environment, stable egress IP, persistent session, two-way sync. Secret files stay out of sync by default; conflicts are shown side by side.

To be clear: **nothing can guarantee you won’t get banned.** This only reduces environment risk.

```
curl -fsSL https://bestcodex.app/install.sh | sh
```

https://bestcodex.app/en

Unsigned beta. Independent, not affiliated with OpenAI/Anthropic. AGPL fork of Codex++ for the desktop shell.
