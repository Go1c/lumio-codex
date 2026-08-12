# Lumio Codex

<p align="center">
  <img src="assets/brand/lumio-icon.png" alt="Lumio Codex icon" width="160">
</p>

<p align="center">
  <a href="README.md">中文</a> | English
</p>

<p align="center">
  <img alt="License" src="https://img.shields.io/github/license/Go1c/lumio-codex">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-2024-orange">
  <img alt="Tauri" src="https://img.shields.io/badge/Tauri-2.x-24C8DB">
</p>

Lumio Codex is a lightweight desktop client for LumioAPI users. It detects an already installed official Codex or ChatGPT desktop app, handles Lumio account onboarding, shows balance and plan status, configures the Responses endpoint, and hands off to the official app so users keep the native Codex model picker.

The production API is fixed at `https://api.lumio.games/`; the marketing site is `https://lumio.games/`. Users never need to enter a Base URL or API key manually.

**Ops / release manuals (Chinese):** start at [docs/ops/README.md](docs/ops/README.md) (local build, website deploy, GitHub releases, Sub2API backend, maintenance).

> **Development / release status:** `publish` is the integration branch. Account flows, config takeover, offline launch, the marketing site, pay-via-browser, and GitHub update reminders are in place. Distribution is still primarily `internal-unsigned` builds. No public production installer ships before Apple Developer ID signing and notarization, Windows code signing, and a rollback drill (see CI `Public release gate`).

## Product flow

The production flow is intentionally fixed:

1. Detect the official Codex or ChatGPT app, with manual path selection as a fallback.
2. Display current terms of service, privacy policy, usage policy, and regional notice.
3. Use Sub2API email verification, password registration or login, and 2FA.
4. Reuse or create the account-scoped `Lumio Codex Desktop` API key.
5. Configure LumioAPI, the Responses protocol, the model catalog, and the server default model.
6. Show balance, trial credit, and plan status.
7. Launch the official Codex app and keep its native model selector for later switches.
8. Open `/payment` in the system browser through a one-time sign-in handoff, without another website login.

Lumio Codex **does not download, modify, or bundle the official Codex or ChatGPT application**. Install a supported desktop app from an official OpenAI channel first.

## Supported platforms

| Platform | Architecture | Current artifact |
| --- | --- | --- |
| Windows | x64 | NSIS installer and portable ZIP |
| macOS | Apple Silicon arm64 | DMG |
| macOS | Intel x64 | DMG |

Windows installs per user under `%LOCALAPPDATA%\Programs\Lumio Codex` without administrator privileges. The macOS DMG exposes one `Lumio Codex.app`; its launch helper stays inside the app bundle.

## Internal test artifacts

The GitHub Actions workflow named `Internal unsigned build artifacts` retains internal packages for 14 days:

- `LumioCodex-<version>-windows-x64-setup-internal-unsigned.exe`
- `LumioCodex-<version>-windows-x64-portable-internal-unsigned.zip`
- `LumioCodex-<version>-macos-arm64-internal-unsigned.dmg`
- `LumioCodex-<version>-macos-x64-internal-unsigned.dmg`

These artifacts do not carry production code signatures and are only for controlled testing. Do not mirror them as stable public builds or weaken operating-system security controls to distribute them more broadly.

After production release is enabled, [GitHub Releases](https://github.com/Go1c/lumio-codex/releases) is the sole authority for versions, tags, checksums, and signed artifacts. The S3 HTTPS download origin only mirrors that exact verified release set. Until signing prerequisites are available, the public release workflow fails closed.

## Product boundaries

Lumio compact mode exposes no Provider, Base URL, key, protocol, multi-provider, script, session-enhancement, Stepwise, Goals, MCP, Skill, Plugin, or injection configuration. The first version also contains no embedded payment UI, third-party OAuth, invitation system, or device-management system.

Reusable upstream implementation may remain in the source tree for synchronization, but the Lumio entry point registers only the compact command surface. Hidden legacy modules are not part of the user-facing product contract.

## Security and privacy

- Tokens and API keys are stored in macOS Keychain or Windows Credential Manager. Logs, crash reports, and UI copy only show redacted values.
- Before first takeover, Lumio snapshots Codex configuration and later merges only Lumio-owned provider fields. Signing out removes credentials and restores that snapshot.
- Telemetry is off by default. If explicitly enabled, it is limited to version, platform, launch stage, and redacted error codes.
- Email addresses, API keys, prompts, source code, file paths, and request content are not telemetry fields.
- During a service outage, a signed-in user with valid cached local configuration may still launch Codex; registration, account refresh, and payment are reported unavailable.

Production secrets, signing credentials, S3 credentials, and deployment configuration must never enter this repository.

## Local development

Use an existing Node.js 22 installation, stable Rust, the Tauri 2 platform prerequisites, and an official desktop app for end-to-end testing. Never place real credentials in tests or commits.

```bash
git clone https://github.com/Go1c/lumio-codex.git
cd lumio-codex/apps/codex-plus-manager
npm ci
npm run check
npm test
npm run vite:build

cd ../../
cargo test -p codex-plus-core --test lumio_product --test installers
cargo test -p codex-plus-manager
cargo check -p codex-plus-manager -p codex-plus-launcher
```

Build local internal binaries:

```bash
cd apps/codex-plus-manager
npm run build
```

Repository layout:

```text
apps/codex-plus-manager/       Lumio Codex Tauri and React client
apps/codex-plus-launcher/      Internal launch helper
crates/codex-plus-core/        Cross-platform detection, config, and launch foundations
crates/codex-plus-data/        Local data layer
assets/brand/                  Brand source and transparent-padded derivative
scripts/installer/windows/     Windows NSIS internal packaging
scripts/installer/macos/       macOS DMG internal packaging
```

## License, upstream, and third-party notices

This repository is published under `AGPL-3.0-only`; see [LICENSE](LICENSE) for the complete terms. If you distribute a modified version or offer it to users over a network, you must provide corresponding source code as required by GNU AGPL v3.0.

Lumio Codex is an AGPL fork of [`BigPizzaV3/CodexPlusPlus`](https://github.com/BigPizzaV3/CodexPlusPlus), with upstream attribution and synchronization preserved. See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for third-party code and asset notices. Rebranding does not remove upstream license or attribution obligations.

Lumio Codex is an independent project and is not affiliated with, sponsored by, or endorsed by OpenAI. OpenAI, ChatGPT, Codex, and related names and marks are trademarks of their respective owners. This project grants no rights to the official application, those trademarks, or third-party content.
