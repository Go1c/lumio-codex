# Lumio Shell and Branding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use subagent-driven-development to implement this plan task-by-task (hosts without subagents: its Inline Fallback section). Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the exposed Codex++ manager surface with a branded, minimal Lumio Codex shell while preserving reusable detection and launch internals for later tasks.

**Architecture:** Add isolated `lumio` modules instead of extending the existing 9k-line `App.tsx`. The React entry renders only `LumioApp`, and the Tauri builder registers only Lumio commands; legacy modules remain compile-time implementation details but are unreachable from the product surface.

**Tech Stack:** Rust 2024, Tauri 2, React 19, TypeScript 5.8, Vite 6, Node test runner, existing LumioAPI raster logo.

## Global Constraints

- Product name is exactly `Lumio Codex`; production API base is exactly `https://api.lumio.games/`.
- Bundle identifier is `games.lumio.codex`.
- Preserve `AGPL-3.0-only`, `THIRD_PARTY_NOTICES.md`, upstream attribution, and the statement that the official Codex app is not bundled.
- Expose no Provider, Base URL, Key, protocol, multi-provider, script, Stepwise, Goals, MCP, Skill, Plugin, injection, or session-enhancement UI/command.
- Keep telemetry disabled and do not add network calls in this plan.
- Do not modify or delete existing user-owned `.spec/`, `AGENTS.md`, `CLAUDE.md`, or `.agents/` worktree changes.
- No dependency installation, public push, release publication, or signing action belongs to this plan.

---

### Task 1: Product constants and branded paths

**Files:**
- Create: `crates/codex-plus-core/src/lumio/mod.rs`
- Create: `crates/codex-plus-core/src/lumio/product.rs`
- Modify: `crates/codex-plus-core/src/lib.rs`
- Test: `crates/codex-plus-core/tests/lumio_product.rs`

**Interfaces:**
- Consumes: `directories::ProjectDirs` already available through the workspace.
- Produces: `PRODUCT_NAME`, `BUNDLE_IDENTIFIER`, `API_BASE_URL`, `DESKTOP_KEY_NAME`, `project_dirs()`, `state_dir()`, `cache_dir()`, and `log_dir()`.

- [ ] **Step 1: Write the failing product contract test**

```rust
use codex_plus_core::lumio::product::{
    API_BASE_URL, BUNDLE_IDENTIFIER, DESKTOP_KEY_NAME, PRODUCT_NAME, project_dirs,
};

#[test]
fn lumio_product_contract_is_stable() {
    assert_eq!(PRODUCT_NAME, "Lumio Codex");
    assert_eq!(BUNDLE_IDENTIFIER, "games.lumio.codex");
    assert_eq!(API_BASE_URL, "https://api.lumio.games/");
    assert_eq!(DESKTOP_KEY_NAME, "Lumio Codex Desktop");
    let dirs = project_dirs().expect("platform project directories");
    assert!(!dirs.data_local_dir().to_string_lossy().contains("Codex++"));
}
```

- [ ] **Step 2: Run the focused test and verify the missing module failure**

Run: `cargo test -p codex-plus-core --test lumio_product`

Expected: FAIL because `codex_plus_core::lumio` does not exist.

- [ ] **Step 3: Add immutable product constants and path helpers**

```rust
// crates/codex-plus-core/src/lumio/product.rs
use std::path::PathBuf;

pub const PRODUCT_NAME: &str = "Lumio Codex";
pub const BUNDLE_IDENTIFIER: &str = "games.lumio.codex";
pub const API_BASE_URL: &str = "https://api.lumio.games/";
pub const DESKTOP_KEY_NAME: &str = "Lumio Codex Desktop";

pub fn project_dirs() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("games", "Lumio", PRODUCT_NAME)
}

pub fn state_dir() -> Option<PathBuf> {
    project_dirs().map(|dirs| dirs.data_local_dir().join("state"))
}

pub fn cache_dir() -> Option<PathBuf> {
    project_dirs().map(|dirs| dirs.cache_dir().to_path_buf())
}

pub fn log_dir() -> Option<PathBuf> {
    project_dirs().map(|dirs| dirs.data_local_dir().join("logs"))
}
```

```rust
// crates/codex-plus-core/src/lumio/mod.rs
pub mod product;
```

Add `pub mod lumio;` to `crates/codex-plus-core/src/lib.rs`.

- [ ] **Step 4: Run the focused test**

Run: `cargo test -p codex-plus-core --test lumio_product`

Expected: PASS, one test.

- [ ] **Step 5: Commit the isolated contract**

```bash
git add crates/codex-plus-core/src/lib.rs crates/codex-plus-core/src/lumio crates/codex-plus-core/tests/lumio_product.rs
git commit -m "feat(lumio): add product contract"
```

### Task 2: Frontend state model

**Files:**
- Create: `apps/codex-plus-manager/src/lumio/types.ts`
- Create: `apps/codex-plus-manager/src/lumio/state.ts`
- Test: `apps/codex-plus-manager/src/lumio/state.test.ts`

**Interfaces:**
- Consumes: no Tauri or network implementation.
- Produces: `LumioPhase`, `LumioBootstrap`, `LumioAccountSummary`, `LumioState`, `LumioEvent`, `initialLumioState()`, and `reduceLumioState()`.

- [ ] **Step 1: Write reducer tests for the allowed product phases**

```ts
import assert from "node:assert/strict";
import test from "node:test";
import { initialLumioState, reduceLumioState } from "./state";

test("bootstrap without account enters signed-out", () => {
  const next = reduceLumioState(initialLumioState(), {
    type: "bootstrapped",
    payload: {
      version: "1.0.0",
      platform: "macos",
      arch: "aarch64",
      codexApp: null,
      account: null,
      telemetryEnabled: false,
      autoUpdateEnabled: true,
    },
  });
  assert.equal(next.phase, "signed-out");
  assert.equal(next.telemetryEnabled, false);
});

test("offline readiness never enables payment or refresh", () => {
  const next = reduceLumioState(initialLumioState(), {
    type: "offline-ready",
    cachedAt: "2026-08-11T00:00:00Z",
  });
  assert.equal(next.phase, "ready-offline");
  assert.equal(next.actions.canLaunch, true);
  assert.equal(next.actions.canRefresh, false);
  assert.equal(next.actions.canPay, false);
});
```

- [ ] **Step 2: Run the reducer test and verify failure**

Run: `cd apps/codex-plus-manager && npm test -- --test-name-pattern='bootstrap|offline readiness'`

Expected: FAIL because `lumio/state.ts` is missing.

- [ ] **Step 3: Implement the typed state machine**

```ts
// apps/codex-plus-manager/src/lumio/types.ts
export type LumioPhase =
  | "bootstrapping"
  | "signed-out"
  | "authenticating"
  | "provisioning"
  | "ready-online"
  | "ready-offline"
  | "needs-repair";

export interface LumioAccountSummary {
  email: string;
  balance: number;
  planLabel: string | null;
}

export interface LumioCodexApp {
  path: string;
  version: string | null;
  source: "automatic" | "manual";
}

export interface LumioBootstrap {
  version: string;
  platform: string;
  arch: string;
  codexApp: LumioCodexApp | null;
  account: LumioAccountSummary | null;
  telemetryEnabled: boolean;
  autoUpdateEnabled: boolean;
}
```

```ts
// apps/codex-plus-manager/src/lumio/state.ts
import type { LumioBootstrap, LumioPhase } from "./types";

export interface LumioActions {
  canLaunch: boolean;
  canRefresh: boolean;
  canPay: boolean;
}

export interface LumioState {
  phase: LumioPhase;
  bootstrap: LumioBootstrap | null;
  telemetryEnabled: boolean;
  autoUpdateEnabled: boolean;
  cachedAt: string | null;
  errorCode: string | null;
  actions: LumioActions;
}

export type LumioEvent =
  | { type: "bootstrapped"; payload: LumioBootstrap }
  | { type: "offline-ready"; cachedAt: string }
  | { type: "repair-required"; errorCode: string };

const disabled: LumioActions = { canLaunch: false, canRefresh: false, canPay: false };

export function initialLumioState(): LumioState {
  return {
    phase: "bootstrapping",
    bootstrap: null,
    telemetryEnabled: false,
    autoUpdateEnabled: true,
    cachedAt: null,
    errorCode: null,
    actions: disabled,
  };
}

export function reduceLumioState(state: LumioState, event: LumioEvent): LumioState {
  if (event.type === "bootstrapped") {
    const signedIn = event.payload.account !== null;
    return {
      ...state,
      phase: signedIn ? "provisioning" : "signed-out",
      bootstrap: event.payload,
      telemetryEnabled: event.payload.telemetryEnabled,
      autoUpdateEnabled: event.payload.autoUpdateEnabled,
      actions: disabled,
    };
  }
  if (event.type === "offline-ready") {
    return {
      ...state,
      phase: "ready-offline",
      cachedAt: event.cachedAt,
      actions: { canLaunch: true, canRefresh: false, canPay: false },
    };
  }
  return { ...state, phase: "needs-repair", errorCode: event.errorCode, actions: disabled };
}
```

- [ ] **Step 4: Run the reducer tests and TypeScript check**

Run: `cd apps/codex-plus-manager && npm test && npm run check`

Expected: all Node tests PASS and TypeScript exits 0.

- [ ] **Step 5: Commit the state model**

```bash
git add apps/codex-plus-manager/src/lumio
git commit -m "feat(lumio): add desktop state model"
```

### Task 3: Narrow Tauri bootstrap command surface

**Files:**
- Create: `apps/codex-plus-manager/src-tauri/src/lumio_commands.rs`
- Replace exposed builder logic in: `apps/codex-plus-manager/src-tauri/src/lib.rs`
- Simplify: `apps/codex-plus-manager/src-tauri/src/main.rs`
- Test: `apps/codex-plus-manager/src-tauri/tests/lumio_command_surface.rs`

**Interfaces:**
- Consumes: `codex_plus_core::app_paths::resolve_codex_app_dir(None)` and `codex_app_version()`.
- Produces: Tauri command `lumio_bootstrap() -> LumioCommandResult<LumioBootstrapPayload>` and the only initial handler list: `lumio_bootstrap`, `lumio_hide_to_tray`, `lumio_exit_app`.

- [ ] **Step 1: Write a source-level allowlist regression test**

```rust
use std::fs;
use std::path::PathBuf;

#[test]
fn lumio_builder_does_not_register_codex_plus_enhancement_commands() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(root.join("src/lib.rs")).expect("lib.rs");
    assert!(source.contains("lumio_commands::lumio_bootstrap"));
    for forbidden in [
        "commands::apply_dream_skin",
        "commands::refresh_script_market",
        "commands::repair_plugin_marketplace",
        "commands::apply_relay_injection",
        "commands::list_local_sessions",
    ] {
        assert!(!source.contains(forbidden), "registered forbidden command: {forbidden}");
    }
}
```

- [ ] **Step 2: Run the command-surface test and verify failure**

Run: `cargo test -p codex-plus-manager --test lumio_command_surface`

Expected: FAIL because the current builder registers forbidden commands.

- [ ] **Step 3: Implement the bootstrap payload and result envelope**

```rust
// apps/codex-plus-manager/src-tauri/src/lumio_commands.rs
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LumioCommandResult<T> {
    pub ok: bool,
    pub error_code: Option<String>,
    pub payload: T,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LumioCodexAppPayload {
    pub path: String,
    pub version: Option<String>,
    pub source: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LumioBootstrapPayload {
    pub version: String,
    pub platform: String,
    pub arch: String,
    pub codex_app: Option<LumioCodexAppPayload>,
    pub account: Option<serde_json::Value>,
    pub telemetry_enabled: bool,
    pub auto_update_enabled: bool,
}

#[tauri::command]
pub fn lumio_bootstrap() -> LumioCommandResult<LumioBootstrapPayload> {
    let codex_app = codex_plus_core::app_paths::resolve_codex_app_dir(None).map(|path| {
        LumioCodexAppPayload {
            version: codex_plus_core::app_paths::codex_app_version(&path),
            path: path.to_string_lossy().into_owned(),
            source: "automatic",
        }
    });
    LumioCommandResult {
        ok: true,
        error_code: None,
        payload: LumioBootstrapPayload {
            version: env!("CARGO_PKG_VERSION").to_string(),
            platform: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            codex_app,
            account: None,
            telemetry_enabled: false,
            auto_update_enabled: true,
        },
    }
}
```

- [ ] **Step 4: Replace the Tauri builder with the Lumio allowlist**

`lib.rs` must expose `pub mod lumio_commands;`, create a single window titled `Lumio Codex`, keep only Show/Quit tray items, and register exactly:

```rust
.invoke_handler(tauri::generate_handler![
    lumio_commands::lumio_bootstrap,
    lumio_hide_to_tray,
    lumio_exit_app,
])
```

`main.rs` must contain only the Windows subsystem attribute and `codex_plus_manager_lib::run();`; remove URL import and Dream Skin argument processing.

- [ ] **Step 5: Run Tauri tests and compile checks**

Run: `cargo test -p codex-plus-manager --test lumio_command_surface`

Expected: PASS.

Run: `cargo check -p codex-plus-manager`

Expected: exits 0 without registering legacy commands.

- [ ] **Step 6: Commit the command boundary**

```bash
git add apps/codex-plus-manager/src-tauri/src apps/codex-plus-manager/src-tauri/tests/lumio_command_surface.rs
git commit -m "feat(lumio): restrict desktop command surface"
```

### Task 4: Minimal branded React shell

**Files:**
- Create: `apps/codex-plus-manager/src/LumioApp.tsx`
- Create: `apps/codex-plus-manager/src/lumio/invoke.ts`
- Create: `apps/codex-plus-manager/src/lumio-shell.css`
- Modify: `apps/codex-plus-manager/src/main.tsx`
- Test: `apps/codex-plus-manager/src/lumio/shell-copy.test.ts`

**Interfaces:**
- Consumes: `lumio_bootstrap` and the state model from Task 2.
- Produces: the only rendered root `LumioApp`, with Home and Settings navigation and no forbidden copy.

- [ ] **Step 1: Write copy and command-name regression tests**

```ts
import assert from "node:assert/strict";
import test from "node:test";
import { LUMIO_BOOTSTRAP_COMMAND, visibleShellLabels } from "./invoke";

test("shell invokes only the Lumio bootstrap command", () => {
  assert.equal(LUMIO_BOOTSTRAP_COMMAND, "lumio_bootstrap");
});

test("shell copy excludes Codex++ enhancement surfaces", () => {
  const copy = visibleShellLabels.join(" ").toLowerCase();
  for (const forbidden of ["provider", "base url", "api key", "stepwise", "mcp", "plugin", "dream skin"]) {
    assert.equal(copy.includes(forbidden), false);
  }
});
```

- [ ] **Step 2: Run the focused test and verify failure**

Run: `cd apps/codex-plus-manager && npm test -- --test-name-pattern='shell'`

Expected: FAIL because `lumio/invoke.ts` is missing.

- [ ] **Step 3: Add the invoke adapter and visible label inventory**

```ts
// apps/codex-plus-manager/src/lumio/invoke.ts
import { invoke } from "@tauri-apps/api/core";
import type { LumioBootstrap } from "./types";

export const LUMIO_BOOTSTRAP_COMMAND = "lumio_bootstrap";
export const visibleShellLabels = [
  "账户状态",
  "余额与套餐",
  "连接状态",
  "默认模型",
  "充值",
  "启动 Codex",
  "开机启动",
  "自动更新",
  "官方应用路径",
  "遥测",
  "日志导出",
  "配置恢复",
] as const;

interface CommandResult<T> {
  ok: boolean;
  errorCode: string | null;
  payload: T;
}

export async function loadLumioBootstrap(): Promise<LumioBootstrap> {
  const result = await invoke<CommandResult<LumioBootstrap>>(LUMIO_BOOTSTRAP_COMMAND);
  if (!result.ok) throw new Error(result.errorCode ?? "BOOTSTRAP_FAILED");
  return result.payload;
}
```

- [ ] **Step 4: Build `LumioApp` around semantic sections**

`LumioApp.tsx` must:

- call `loadLumioBootstrap()` once in `useEffect`;
- render a signed-out hero while `account` is null;
- render account, balance/plan, connection, model, payment, and launch cards only through typed state;
- render Settings with only the six approved controls;
- keep unavailable actions disabled with explicit status text instead of fake success;
- use the existing Lumio logo at `/lumio-icon.png` and text `Lumio Codex`.

Replace `main.tsx` imports with:

```tsx
import { createRoot } from "react-dom/client";
import { LumioApp } from "./LumioApp";
import "./lumio-shell.css";
import "@fontsource/jetbrains-mono";

const app = document.getElementById("app");
if (app instanceof HTMLElement) createRoot(app).render(<LumioApp />);
```

- [ ] **Step 5: Verify tests, types, and production bundle**

Run: `cd apps/codex-plus-manager && npm test && npm run check && npm run vite:build`

Expected: all tests PASS, TypeScript exits 0, Vite emits `dist/` without importing `./App` from `main.tsx`.

- [ ] **Step 6: Commit the shell**

```bash
git add apps/codex-plus-manager/src apps/codex-plus-manager/index.html
git commit -m "feat(lumio): add minimal branded desktop shell"
```

### Task 5: Brand manifests, binaries, icons, and installers

**Files:**
- Create: `assets/brand/lumio-icon.png` from `/Users/cui/Sites/sub2api/frontend/public/logo.png`
- Generate: `apps/codex-plus-manager/src-tauri/icons/*` from the brand source
- Modify: `Cargo.toml`
- Modify: `apps/codex-plus-manager/package.json`
- Modify: `apps/codex-plus-manager/package-lock.json` through `npm install --package-lock-only --ignore-scripts` only if the package name/version lock entry requires regeneration
- Modify: `apps/codex-plus-manager/src-tauri/Cargo.toml`
- Modify: `apps/codex-plus-launcher/Cargo.toml`
- Modify: `apps/codex-plus-manager/src-tauri/tauri.conf.json`
- Create: `scripts/installer/windows/LumioCodex.nsi`
- Delete: `scripts/installer/windows/CodexPlusPlus.nsi`
- Modify: `scripts/installer/macos/package-dmg.sh`
- Modify: `apps/codex-plus-launcher/build.rs`
- Modify: `apps/codex-plus-manager/src-tauri/build.rs`
- Test: `crates/codex-plus-core/tests/installers.rs`
- Test: `apps/codex-plus-manager/src-tauri/tests/windows_subsystem.rs`

**Interfaces:**
- Consumes: product constants from Task 1 and the existing LumioAPI logo.
- Produces: binaries `lumio-codex` and `lumio-codex-launcher`, bundle ID `games.lumio.codex`, four future release artifact names rooted at `LumioCodex-<version>`.

- [ ] **Step 1: Update installer assertions first**

Change installer tests to assert these exact tokens and reject legacy public branding:

```rust
assert!(windows_installer.contains("Name \"Lumio Codex\""));
assert!(windows_installer.contains("LumioCodex-${VERSION}-windows-x64-setup.exe"));
assert!(macos_installer.contains("Lumio Codex.app"));
assert!(macos_installer.contains("games.lumio.codex"));
for legacy in ["CodexPlusPlus-${VERSION}", "com.bigpizzav3.codexplusplus"] {
    assert!(!windows_installer.contains(legacy));
    assert!(!macos_installer.contains(legacy));
}
```

- [ ] **Step 2: Run installer tests and verify failure**

Run: `cargo test -p codex-plus-core --test installers`

Expected: FAIL on legacy names.

- [ ] **Step 3: Copy the approved source logo and generate platform icons**

Copy `/Users/cui/Sites/sub2api/frontend/public/logo.png` byte-for-byte to `assets/brand/lumio-icon.png` and `apps/codex-plus-manager/public/lumio-icon.png`.

Run: `cd apps/codex-plus-manager && npx tauri icon ../../assets/brand/lumio-icon.png --output src-tauri/icons`

Expected: Tauri generates PNG, ICO, and ICNS assets from the same source; no network install prompt appears.

- [ ] **Step 4: Apply exact brand metadata**

- Workspace repository: `https://github.com/Go1c/lumio-codex`.
- npm package name: `lumio-codex`.
- Tauri product: `Lumio Codex`; identifier: `games.lumio.codex`; title: `Lumio Codex`.
- Manager binary: `lumio-codex`; launcher binary: `lumio-codex-launcher`.
- Windows install directory: `$LOCALAPPDATA\Programs\Lumio Codex`; publisher: `Lumio`.
- macOS bundle: one visible `Lumio Codex.app` plus its internal launcher companion; no Dream Skin URL schemes.
- Internal unsigned output names end with `-internal-unsigned` until the release plan enables signing.

The macOS packaging script must use a validated task-specific `mktemp -d` staging directory and trap cleanup; it must not execute `rm -rf` against a fixed repository directory.

- [ ] **Step 5: Verify brand tests and compile metadata**

Run: `cargo test -p codex-plus-core --test installers`

Expected: PASS.

Run: `cargo check -p codex-plus-manager -p codex-plus-launcher`

Expected: exits 0 with renamed binaries.

- [ ] **Step 6: Commit generated assets with their source**

```bash
git add Cargo.toml assets/brand apps/codex-plus-manager apps/codex-plus-launcher scripts/installer
git commit -m "feat(brand): rebrand desktop packaging as Lumio Codex"
```

### Task 6: Public README and attribution

**Files:**
- Modify: `README.md`
- Modify: `README_EN.md`
- Verify: `LICENSE`
- Verify: `THIRD_PARTY_NOTICES.md`

**Interfaces:**
- Consumes: final product and artifact names from Tasks 1 and 5.
- Produces: public source documentation with no production secret or unsigned-public-release claim.

- [ ] **Step 1: Replace README product documentation**

Both READMEs must contain:

- product purpose and supported macOS/Windows targets;
- official Codex/ChatGPT prerequisite and non-bundling statement;
- LumioAPI endpoint stated as product behavior, without credentials;
- internal unsigned build warning;
- source/build instructions using existing toolchains;
- `Go1c/lumio-codex` release links;
- AGPL source-availability statement;
- upstream attribution to `BigPizzaV3/CodexPlusPlus`;
- OpenAI/Codex/ChatGPT trademark disclaimer.

Remove sponsor tables, Dream Skin/Stepwise/Provider instructions, old installer names, and claims that those surfaces exist in Lumio mode.

- [ ] **Step 2: Run documentation and brand scans**

Run: `rg -n "BigPizzaV3/CodexPlusPlus|AGPL-3.0|official Codex|官方 Codex|not bundled|不捆绑" README.md README_EN.md LICENSE THIRD_PARTY_NOTICES.md`

Expected: attribution and license/non-bundling statements are present.

Run: `rg -n "Codex\+\+ Manager|CodexPlusPlus-.*(setup|dmg)|com\.bigpizzav3\.codexplusplus" README.md README_EN.md apps/codex-plus-manager/src-tauri/tauri.conf.json scripts/installer`

Expected: no matches.

- [ ] **Step 3: Commit documentation separately**

```bash
git add README.md README_EN.md
git commit -m "docs: document Lumio Codex distribution"
```

### Task 7: Shell milestone verification

**Files:**
- Verify only; do not add production behavior in this task.

**Interfaces:**
- Consumes: all previous task outputs.
- Produces: evidence that the branded shell is a buildable, restricted foundation for account integration.

- [ ] **Step 1: Run Rust formatting and tests**

Run: `cargo fmt --all -- --check`

Expected: exits 0.

Run: `cargo test -p codex-plus-core --test lumio_product --test installers && cargo test -p codex-plus-manager`

Expected: all selected tests PASS.

- [ ] **Step 2: Run frontend verification**

Run: `cd apps/codex-plus-manager && npm run check && npm test && npm run vite:build`

Expected: check, tests, and build all exit 0.

- [ ] **Step 3: Audit the exposed surface**

Run: `rg -n "Provider|Base URL|API Key|Stepwise|Goals|MCP|Skill|Plugin|Dream Skin|脚本|注入" apps/codex-plus-manager/src/LumioApp.tsx apps/codex-plus-manager/src/lumio-shell.css apps/codex-plus-manager/src-tauri/src/lib.rs`

Expected: no forbidden product-surface matches; `API Key` may occur only in non-rendered test assertions that explicitly reject it.

- [ ] **Step 4: Record milestone status without claiming the full product is complete**

Update the corresponding `.spec/tasks/` card with command outputs and leave the overall Lumio client goal in progress. This shell milestone does not satisfy authentication, secure storage, config takeover, payment handoff, or signed release requirements.
