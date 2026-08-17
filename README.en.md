[中文](README.md) · **English**

# BestCodex

**One launcher, two ways to work.** Install once, sign in once, and you get two tabs in a single window:

- **Codex** — use the **official OpenAI Codex** app with zero configuration. No base URL to fill in, no API key to paste. Signing in writes the local connection config for you. The official app is never bundled or modified.
- **Claude** — run the **official Claude Code** on **your own server**: an isolated environment, a stable outbound IP, and persistent sessions. Files sync both ways with your local machine.

Website: [bestcodex.app](https://bestcodex.app) · Guides: [bestcodex.app/en/guides](https://bestcodex.app/en/guides) · Help centre (Chinese): [bestcodex.app/help](https://bestcodex.app/help)

> BestCodex is an independent project. It is **not affiliated with, sponsored by, or endorsed by OpenAI or Anthropic**. It is also unrelated to any similarly named third-party API relay service — this project's only website is `bestcodex.app`.

## What problem it solves

**Codex setup friction.** Getting the official Codex app talking to a provider means editing config files, filling in a base URL, and pasting an API key. BestCodex does that step for you: sign in, and it writes only the connection config it manages — after taking a backup you can restore at any time. What launches afterwards is the official Codex app itself.

**Claude Code account risk and lost context.** Ban risk comes largely from the *environment*: shared egress IPs, network locations that jump around, and coding sessions mixed in with everything else on your machine. Running Claude Code on a dedicated server with a stable IP reduces that exposure, and a persistent session means a dropped connection no longer costs you the conversation context. To be clear: **no approach can guarantee you won't be banned** — the terms of service and the risk rules belong to Anthropic, and any third party can only reduce risk, not remove it.

Your code stays between your machine and your server. We store account and workspace metadata only, never your source. Secret files are excluded from sync by default, and conflicts are shown side by side for you to resolve rather than being silently overwritten.

## Install

Supported platforms: **macOS 13+ (Apple silicon and Intel are separate builds)** and **Windows 10/11 64-bit**. One installer covers both tabs. Download from [bestcodex.app](https://bestcodex.app).

> **These are unsigned beta builds.** On macOS, Gatekeeper flags the quarantine attribute and reports *"BestCodex is damaged and can't be opened"* — the app is not actually damaged. Move it to `/Applications`, then clear the attribute:
>
> ```bash
> xattr -cr "/Applications/BestCodex.app"
> ```
>
> If it still won't open, Control-click the icon and choose **Open** to confirm explicitly. On Windows, SmartScreen will warn you; verify the source and continue. See [the unsigned-build notes](https://bestcodex.app/help/unsigned) for details.

The official Codex app is **not bundled** in the installer. Install it separately when you need it.

## Relationship to Codex++

The `codex/` directory in this repository is an **AGPL-3.0 fork** of [`BigPizzaV3/CodexPlusPlus`](https://github.com/BigPizzaV3/CodexPlusPlus) (Codex++), with upstream attribution and licensing preserved.

The two projects aim at different people:

| | Codex++ (upstream) | BestCodex |
|---|---|---|
| Built for | Advanced users who want to deeply modify Codex | People who want the official Codex working quickly |
| Focus | Provider switching, protocol translation, plugin unlocking, UI enhancements | Zero-config setup, account and balance handling, works out of the box |
| Claude Code | Not covered | Built-in Claude tab: runs on your own server with two-way sync |

**If you want provider switching or deep customization, use upstream Codex++ directly.** Feature sets will diverge over time — don't assume parity in either direction.

## Repository layout

This is a monorepo. The three product areas build independently and do not import from each other:

| Path | Contents |
| --- | --- |
| `codex/` | **The desktop launcher** (Rust workspace + Tauri 2 + React 19). The AGPL fork of Codex++, including product docs in `codex/docs/` |
| `cchaven/` | **What powers the Claude tab** (directory still named cchaven / CC): remote Claude Code execution and two-way sync — a Go control plane, web app, admin console, Tauri desktop client, and the sync agent |
| `web/` | **The websites**: `apps/bestcodex` (the product site at `bestcodex.app`) and `apps/portal` (accounts), sharing `packages/ui` and `packages/auth` |
| `.spec/` | Agent conventions and the project knowledge base (single source of truth; see [`AGENTS.md`](AGENTS.md)) |

`codex/` and `cchaven/` are self-contained Rust workspace / npm projects — build and test them from within their own directories.

## Building the website

```bash
cd web
npm ci
npm run build                                  # both sites
npm run build --workspace @lumio/bestcodex     # product site only
```

The product site is **prerendered at build time**: every route is rendered to static HTML so that crawlers and AI engines — most of which do not execute JavaScript — can read the content. The build also emits `sitemap.xml`, `llms.txt`, plain-Markdown mirrors of each article, and a real `404.html`.

Output lands in `web/apps/bestcodex/dist/` and `web/apps/portal/dist/` as static sites. See [`web/README.md`](web/README.md) for development commands and [`docs/ops/`](docs/ops/README.md) for deployment.

## Licensing

- `codex/` is an **AGPL-3.0-only** fork of Codex++: see [`codex/LICENSE`](codex/LICENSE) and [`codex/THIRD_PARTY_NOTICES.md`](codex/THIRD_PARTY_NOTICES.md).
- `cchaven/` and `web/` do not currently declare a separate license.
- OpenAI, ChatGPT, Codex, Claude, and Anthropic are trademarks of their respective owners. Official applications must be installed separately.

## Where to go next

- Desktop launcher: [`codex/README.md`](codex/README.md)
- Claude tab internals: [`cchaven/README.md`](cchaven/README.md)
- Websites: [`web/README.md`](web/README.md) · SEO/GEO operations: [`docs/seo-operations.md`](docs/seo-operations.md)
- Agent collaboration conventions: [`AGENTS.md`](AGENTS.md)
