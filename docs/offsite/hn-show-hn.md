# Show HN: BestCodex — one launcher for official Codex and Claude Code on your own server

**Suggested title:** Show HN: BestCodex – zero-config official Codex, plus Claude Code on your own server

**Where:** news.ycombinator.com → Submit (Show HN)

---

BestCodex is a desktop launcher (macOS / Windows). Two tabs, one sign-in:

1. **Codex** — writes the local connection config so you can launch the *official* OpenAI Codex app. No base URL, no API key form. It does not bundle or modify the official app.
2. **Claude** — runs official Claude Code on *your* server (stable IP, isolated environment, persistent session, two-way file sync). This lowers the usual environment-based ban signals. It cannot guarantee you won't be banned; the rules belong to Anthropic.

Install:

```
# macOS
curl -fsSL https://bestcodex.app/install.sh | sh

# Windows (PowerShell)
irm https://bestcodex.app/install.ps1 | iex
```

Or download from https://bestcodex.app

Current builds are unsigned betas. On macOS, Gatekeeper may say the app is “damaged”; the install script clears the quarantine attribute. The desktop app is an AGPL-3.0 fork of Codex++ (https://github.com/BigPizzaV3/CodexPlusPlus). Independent project — not affiliated with OpenAI or Anthropic. Unrelated to any similarly named API relay.

Site: https://bestcodex.app  
English: https://bestcodex.app/en  
Repo: https://github.com/Go1c/lumio-codex
