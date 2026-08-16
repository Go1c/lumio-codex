# 官方 Codex 安装位置选择 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use subagent-driven-development to implement this plan task-by-task (hosts without subagents: its Inline Fallback section). Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 首次安装官方 Codex 前让用户选择安装目录——macOS 任意目录拷 .app；Windows 默认保持 MSIX 侧载、提供并列的「选择目录安装」走便携解压；安装成功后清理安装包缓存；所选路径持久化并接入启动检测。

**Architecture:** 目的地作为 `Option<PathBuf>` 从 IPC 一路传进安装管线：`plan_official_app` 据 presence 决定 Windows 路线（有目的地 → 便携，无视侧载探测），`live_install` 把目的地交给便携解压 / macOS 拷贝；安装成功后删安装包、仅当用户选了目录时把最终路径落 `state_dir()/official-app-path.json`，`detect_existing_app` 优先读它（失效回落自动扫描）。

**Tech Stack:** Rust（codex-plus-core / codex-plus-manager，toml_edit 无涉及）、React 19 + TS（复用 `@tauri-apps/plugin-dialog`，零新依赖）。

## Global Constraints

- 零新依赖：不改 `Cargo.toml` / `package.json`。
- 安全防线不降级：便携路线仍强制 Authenticode 钉选（D-21 三态规则原样）；`Mismatch` 拒、`Unavailable` 仅侧载放行的语义不变。
- lumio 壳文案为硬编码中文常量（跟 `HomeView` 现状一致，不走 i18n-en）。
- IPC payload 不含任何密钥材料（契约测试 `command_payloads_never_expose_tokens_or_key_material` 钉住）。
- 收口门槛：`cargo fmt --all -- --check`；`cargo test -p codex-plus-core --lib lumio`；`cargo test -p codex-plus-core --test installers`；`cargo test -p codex-plus-manager`；`apps/codex-plus-manager` 内 `npm run check` + `npm test`；改 `.spec/` 后 `node .spec/tools/spec-lint.mjs`。
- 本轮明确不做（YAGNI，记入 QA known gaps）：剩余空间预检、下载缓存迁移到目标盘、设置页手动路径持久化（D-3 旧坑）、UX 原型新增变体。

---

### Task 1: 计划层——目的地驱动 Windows 路线

**Files:**
- Modify: `codex/crates/codex-plus-core/src/lumio/official_app_install/mod.rs`（`PlanInput` 增字段）
- Modify: `codex/crates/codex-plus-core/src/lumio/official_app_install/plan.rs`（路线决策 + 测试）

**Interfaces:**
- Produces: `PlanInput<'a>` 新增 `pub destination: Option<&'a Path>`；`plan_official_app` 在 `HostPlatform::Windows` 且 `destination.is_some()` 时返回 `InstallRoute::WindowsPortable`（优先于 `windows_sideload_ok` 判断）；macOS 路线不变（`MacosCopyApp`），目的地由安装层消费。

- [x] **Step 1: 写失败测试（plan.rs tests）**

```rust
    #[test]
    fn a_windows_destination_forces_the_portable_route() {
        // 用户选了目录就必须落进那个目录：MSIX 侧载装哪由 Windows 管，唯一能
        // 兑现「选目录」的是便携解压（D-23）。侧载探测可用也不得改写该决定。
        let decision = plan_official_app(PlanInput {
            platform: HostPlatform::Windows,
            arch: HostArch::X64,
            detected_app: None,
            online: true,
            windows_sideload_ok: Some(true),
            destination: Some(Path::new(r"D:\MyApps")),
        })
        .unwrap();
        let InstallDecision::Ready { route, .. } = decision else {
            panic!()
        };
        assert_eq!(route, InstallRoute::WindowsPortable);
    }

    #[test]
    fn a_macos_destination_keeps_the_copy_route() {
        let decision = plan_official_app(PlanInput {
            platform: HostPlatform::Macos,
            arch: HostArch::Arm64,
            detected_app: None,
            online: true,
            windows_sideload_ok: None,
            destination: Some(Path::new("/Volumes/D/Apps")),
        })
        .unwrap();
        let InstallDecision::Ready { route, .. } = decision else {
            panic!()
        };
        assert_eq!(route, InstallRoute::MacosCopyApp);
    }
```

同文件既有 `PlanInput` 构造（6 处）补 `destination: None`。

- [x] **Step 2: 跑测试确认红**

Run: `cd codex && cargo test -p codex-plus-core --lib lumio::official_app_install::plan`
Expected: 编译失败 `error[E0561]: missing field destination`（或等价）。

- [x] **Step 3: 实现**

`mod.rs`：

```rust
pub struct PlanInput<'a> {
    pub platform: HostPlatform,
    pub arch: HostArch,
    pub detected_app: Option<&'a Path>,
    pub online: bool,
    pub windows_sideload_ok: Option<bool>,
    /// 用户选择的安装目录：Windows 上强制便携路线，macOS 上作为 .app 拷贝目标。
    pub destination: Option<&'a Path>,
}
```

`run_official_app_install` 内重建 `PlanInput` 处补 `destination: request.plan.destination`。

`plan.rs`：

```rust
    let route = match input.platform {
        // 用户选了目录 → 便携解压是唯一能兑现目的地的路线（MSIX 装哪由 Windows 管）。
        HostPlatform::Windows if input.destination.is_some() => InstallRoute::WindowsPortable,
        HostPlatform::Windows if input.windows_sideload_ok == Some(false) => {
            InstallRoute::WindowsPortable
        }
        HostPlatform::Windows => InstallRoute::WindowsSideload,
        HostPlatform::Macos => InstallRoute::MacosCopyApp,
    };
```

mod.rs tests 的 `ready_request` 构造补 `destination: None`。

- [x] **Step 4: 跑测试确认绿**

Run: `cargo test -p codex-plus-core --lib lumio::official_app_install`
Expected: 全部 PASS。

- [x] **Step 5: Commit**（本会话不提交，攒一批）

```bash
git add codex/crates/codex-plus-core/src/lumio/official_app_install/
git commit -m "feat(codex): let a chosen destination force the portable install route"
```

---

### Task 2: 安装层——目的地落地、装后清包、路径持久化

**Files:**
- Create: `codex/crates/codex-plus-core/src/lumio/official_app_install/install_path.rs`
- Modify: `codex/crates/codex-plus-core/src/lumio/official_app_install/mod.rs`（声明子模块、签名加参、清包、持久化、detect 接线）
- Modify: `codex/crates/codex-plus-core/src/lumio/official_app_install/macos.rs`（`install_macos_from_dmg` 增目的地参数）

**Interfaces:**
- Consumes: Task 1 的 `PlanInput.destination`。
- Produces:
  - `install_path::saved_install_path(state_dir: &Path) -> Option<PathBuf>`
  - `install_path::save_install_path(state_dir: &Path, path: &Path) -> Result<(), String>`（错误码 `CODEX_APP_INSTALL_FAILED`）
  - `start_official_app_install_with(session_app: Option<PathBuf>, destination: Option<PathBuf>) -> Result<PathBuf, String>`；`start_official_app_install()` 传 `None`
  - `begin_background_install(session_app: Option<PathBuf>, destination: Option<PathBuf>) -> Result<(), String>`
  - `install_macos_from_dmg(dmg: &Path, dest_root: Option<&Path>) -> Result<PathBuf, String>`
  - `detect_existing_app_with(session_app: Option<&Path>, state_dir: Option<&Path>) -> Option<PathBuf>`（公开 wrapper `detect_existing_app` 传 `product::state_dir()`）

- [x] **Step 1: 写失败测试**

`install_path.rs` 内嵌 tests：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_then_load_roundtrips_the_path() {
        let root = tempfile::tempdir().unwrap();
        save_install_path(root.path(), Path::new(r"D:\MyApps\Codex")).unwrap();
        assert_eq!(
            saved_install_path(root.path()),
            Some(PathBuf::from(r"D:\MyApps\Codex"))
        );
    }

    #[test]
    fn blank_or_corrupt_or_missing_records_read_as_none() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(saved_install_path(root.path()), None);
        std::fs::write(root.path().join("official-app-path.json"), "not json").unwrap();
        assert_eq!(saved_install_path(root.path()), None);
        std::fs::write(
            root.path().join("official-app-path.json"),
            r#"{"installPath":"   "}"#,
        )
        .unwrap();
        assert_eq!(saved_install_path(root.path()), None);
    }
}
```

`mod.rs` tests：

```rust
    #[test]
    fn a_successful_install_deletes_the_downloaded_package() {
        // 745MB 安装包装完即删（失败保留供重试）：C 盘峰值是下载瞬时，不常驻。
        let _guard = progress::reset_status_for_tests();
        let pkg = tempfile::tempdir().unwrap();
        let package = pkg.path().join("win-x64.msix");
        std::fs::write(&package, b"pkg").unwrap();

        let path = run_official_app_install(OfficialAppInstallRequest {
            plan: ready_request(None),
            detect: &|| Some(PathBuf::from("/tmp/app")),
            download: &mut |_source| Ok(package.clone()),
            verify: &|_path, _source, _route| Ok(()),
            install: &mut |_path, _route| Ok(PathBuf::from("/tmp/app")),
            write_config: &mut || {},
        })
        .expect("install must succeed");

        assert!(path.is_some() || true); // 语义断言在下一行
        assert!(!package.exists(), "the package must be removed after success");
    }
```

（断言精简为 `assert!(!package.exists())`，删除上面占位行。）

```rust
    #[test]
    fn a_failed_install_keeps_the_package_for_retry() {
        let _guard = progress::reset_status_for_tests();
        let pkg = tempfile::tempdir().unwrap();
        let package = pkg.path().join("win-x64.msix");
        std::fs::write(&package, b"pkg").unwrap();

        let _ = run_official_app_install(OfficialAppInstallRequest {
            plan: ready_request(None),
            detect: &|| None,
            download: &mut |_source| Ok(package.clone()),
            verify: &|_path, _source, _route| Ok(()),
            install: &mut |_path, _route| Err("CODEX_APP_INSTALL_FAILED".to_string()),
            write_config: &mut || {},
        })
        .unwrap_err();

        assert!(package.exists(), "a failed install must keep the package for retry");
    }

    #[test]
    fn detection_prefers_the_saved_destination_and_falls_back_when_stale() {
        let root = tempfile::tempdir().unwrap();
        let fake_app = root.path().join("MyApps").join("Codex");
        std::fs::create_dir_all(&fake_app).unwrap();

        // 无保存记录：与旧行为一致。
        assert!(detect_existing_app_with(None, Some(root.path())).is_none());

        // 保存的是「目录基址」，探测按平台拼出应用本体（Windows：目录本身含 exe
        // 即有效；macOS：拼 Codex.app）。这里以保存值直接可验证为准。
        save_install_path(root.path(), &fake_app).unwrap();
        // fake_app 没有 Codex 可执行文件，normalize 会判无效 → 回落自动扫描（此处为 None）。
        assert!(detect_existing_app_with(None, Some(root.path())).is_none());
    }
```

（第三条测的是「保存但失效 → 回落」；正向命中依赖 `normalize_codex_app_path` 的平台语义，macOS 宿主上用 `xxx.app` 目录可构造正向用例：`std::fs::create_dir_all(root.join("Apps/Codex.app/Contents/MacOS"))` 后保存 `root/Apps`，macOS 探测拼 `Codex.app` 应命中。执行时按宿主补齐。）

- [x] **Step 2: 跑测试确认红**

Run: `cargo test -p codex-plus-core --lib lumio::official_app_install`
Expected: 编译失败（`install_path` / `detect_existing_app_with` / 新参数不存在）。

- [x] **Step 3: 实现**

`install_path.rs`：

```rust
//! 用户自选安装目录的持久化。MSIX 侧载 / 默认路线不写这个文件——它们由自动
//! 探测覆盖；只有「用户选了目录」这种自动探测找不到的安装才需要记住落点，
//! 否则重启后会误判未安装并触发重复安装（D-23，与 D-3 手选丢失同族）。

use std::path::{Path, PathBuf};

const INSTALL_PATH_FILE: &str = "official-app-path.json";

pub fn saved_install_path(state_dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(state_dir.join(INSTALL_PATH_FILE)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let path = value.get("installPath")?.as_str()?;
    let trimmed = path.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

pub fn save_install_path(state_dir: &Path, path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(state_dir).map_err(|_| super::INSTALL_FAILED.to_string())?;
    let payload = serde_json::json!({ "installPath": path.to_string_lossy() });
    crate::settings::atomic_write(
        state_dir.join(INSTALL_PATH_FILE),
        payload.to_string().as_bytes(),
    )
    .map_err(|_| super::INSTALL_FAILED.to_string())
}
```

（`INSTALL_FAILED` 在 `mod.rs` 是私有常量，同为 `official_app_install` 模块树内可见。）

`mod.rs`：

```rust
mod install_path;
```

`run_official_app_install` 的 install 成功分支（`set_succeeded` 之前）加：

```rust
                                // 安装包装完即删；失败路径不进这里，包留给重试。
                                let _ = std::fs::remove_file(&package);
```

`start_official_app_install_with` 签名与结尾：

```rust
pub async fn start_official_app_install_with(
    session_app: Option<PathBuf>,
    destination: Option<PathBuf>,
) -> Result<PathBuf, String> {
    ...
    let result = run_official_app_install(OfficialAppInstallRequest {
        plan: PlanInput {
            platform,
            arch,
            detected_app: detected.as_deref(),
            online: true,
            windows_sideload_ok,
            destination: destination.as_deref(),
        },
        ...
        install: &mut |path, route| live_install(path, route, destination.as_deref()),
        ...
    });
    // 用户选了目录的安装只有这里记住落点，重启后检测才找得到。
    if result.is_ok() && destination.is_some() {
        if let (Some(state), Ok(path)) = (crate::lumio::product::state_dir(), result.as_ref()) {
            let _ = install_path::save_install_path(&state, path);
        }
    }
    result
}
```

`live_install` 与 `begin_background_install`：

```rust
fn live_install(path: &Path, route: InstallRoute, destination: Option<&Path>) -> Result<PathBuf, String> {
    match route {
        InstallRoute::WindowsSideload => install_windows_sideload(path),
        InstallRoute::WindowsPortable => {
            let dest = match destination {
                Some(dest) => dest.to_path_buf(),
                None => windows_portable_dest()?,
            };
            install_windows_portable(path, &dest)
        }
        InstallRoute::MacosCopyApp => install_macos_from_dmg(path, destination),
    }
}

pub fn begin_background_install(
    session_app: Option<PathBuf>,
    destination: Option<PathBuf>,
) -> Result<(), String> {
    ...
        let result = start_official_app_install_with(session_app, destination).await;
    ...
}
```

`detect_existing_app` 拆分：

```rust
pub fn detect_existing_app(session_app: Option<&Path>) -> Option<PathBuf> {
    detect_existing_app_with(session_app, crate::lumio::product::state_dir().as_deref())
}

pub fn detect_existing_app_with(
    session_app: Option<&Path>,
    state_dir: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(path) = session_app {
        if let Some(valid) = valid_manual_app(path) {
            return Some(valid);
        }
    }
    // 用户自选目录优先于自动扫描；失效（卸载/移动）则原样回落（D-23）。
    if let Some(saved) = state_dir.and_then(install_path::saved_install_path) {
        if let Some(valid) = valid_manual_app(&saved) {
            return Some(valid);
        }
    }
    crate::app_paths::resolve_codex_app_dir(None)
        .or_else(crate::app_paths::find_standalone_codex_app_dir)
}
```

`macos.rs`：

```rust
pub fn install_macos_from_dmg(dmg: &Path, dest_root: Option<&Path>) -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    {
        install_macos_from_dmg_live(dmg, dest_root)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (dmg, dest_root);
        Err(INSTALL_FAILED.to_string())
    }
}
```

`install_macos_from_dmg_live` 内：

```rust
    let existing = crate::app_paths::find_macos_codex_app_default();
    let dest = dest_root
        .map(|root| root.join(APP_BUNDLE_NAME))
        .unwrap_or_else(|| choose_macos_dest(existing.as_deref(), system_applications_writable()));
```

- [x] **Step 4: 跑测试确认绿**

Run: `cargo test -p codex-plus-core --lib lumio::official_app_install && cargo test -p codex-plus-core --test installers`
Expected: 全部 PASS。

- [x] **Step 5: Commit**（本会话不提交）

```bash
git add codex/crates/codex-plus-core/src/lumio/official_app_install/
git commit -m "feat(codex): honor a chosen install destination, clean the package, persist the path"
```

---

### Task 3: IPC——命令面接收 destination（防静默丢参）

**Files:**
- Modify: `codex/apps/codex-plus-manager/src-tauri/src/lumio_commands.rs`（`lumio_install_official_app`）
- Test: `codex/apps/codex-plus-manager/src-tauri/tests/lumio_command_surface.rs`

**Interfaces:**
- Consumes: Task 2 的 `begin_background_install(session_app, destination)`。
- Produces: `lumio_install_official_app(session, destination: Option<String>)`——前端 invoke 参数名 `destination`。

- [x] **Step 1: 写失败测试**

```rust
#[test]
fn lumio_install_official_app_accepts_the_destination_argument() {
    // 前端选择目录后传 destination；命令签名一旦漏掉该参数，Tauri 会静默丢弃，
    // 用户选的目录被无视、仍装到默认位置（D-23，同 D-1 的静默丢参坑）。
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(root.join("src/lumio_commands.rs")).expect("lumio_commands.rs");

    let signature = source
        .split_once("pub async fn lumio_install_official_app(")
        .and_then(|(_, rest)| rest.split_once(") -> Result<LumioCommandResult<LumioOfficialAppInstallPayload>"))
        .map(|(signature, _)| signature)
        .expect("lumio_install_official_app signature");
    assert!(
        signature.contains("destination: Option<String>"),
        "lumio_install_official_app must accept destination from the frontend:\n{signature}"
    );

    let body = source
        .split_once("pub async fn lumio_install_official_app(")
        .and_then(|(_, rest)| rest.split_once("pub fn lumio_official_app_status"))
        .map(|(body, _)| body)
        .expect("lumio_install_official_app body");
    assert!(
        body.contains("begin_background_install(session_app, destination)"),
        "the destination must be forwarded to the install pipeline:\n{body}"
    );
}
```

- [x] **Step 2: 跑测试确认红**

Run: `cargo test -p codex-plus-manager --test lumio_command_surface`
Expected: 新测试 FAIL（签名无 destination）。

- [x] **Step 3: 实现**

```rust
#[tauri::command]
pub async fn lumio_install_official_app(
    session: tauri::State<'_, LumioSession>,
    destination: Option<String>,
) -> Result<LumioCommandResult<LumioOfficialAppInstallPayload>, ()> {
    // 空串视同未选择（走默认路线），不做其他猜测式归一。
    let destination = destination
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from);
    let session_app = lock(&session.codex_app).clone();
    if let Some(path) = session_app.as_ref() {
        if official_app_install::manual_path_still_valid(path) {
            official_app_install::note_already_installed(path.clone());
            return result(Ok(official_app_status_payload(Some(false))));
        }
    }
    if let Some(path) = official_app_install::detect_existing_app(session_app.as_deref()) {
        official_app_install::note_already_installed(path);
        return result(Ok(official_app_status_payload(Some(false))));
    }

    match official_app_install::begin_background_install(session_app, destination) {
        Ok(()) => result(Ok(official_app_status_payload(Some(true)))),
        Err(code) => result(Err(code)),
    }
}
```

- [x] **Step 4: 跑测试确认绿**

Run: `cargo test -p codex-plus-manager`
Expected: 全部 PASS。

- [x] **Step 5: Commit**（本会话不提交）

```bash
git add codex/apps/codex-plus-manager/src-tauri/
git commit -m "feat(codex): accept the install destination over IPC"
```

---

### Task 4: 前端——安装位置选择步骤

**Files:**
- Create: `codex/apps/codex-plus-manager/src/lumio/install-destination.ts` + `install-destination.test.ts`
- Modify: `codex/apps/codex-plus-manager/src/lumio/invoke.ts:280`（`installOfficialApp` 带参）
- Modify: `codex/apps/codex-plus-manager/src/lumio/views/HomeView.tsx`（主按钮先开位置选择弹窗）

**Interfaces:**
- Consumes: Task 3 的 IPC 参数 `destination`。
- Produces: `destinationOptions(platform: string): readonly DestinationOption[]`，`DestinationOption = { id: "standard" | "choose"; label: string; note: string | null }`；`installOfficialApp(destination: string | null = null)`。

- [x] **Step 1: 写失败测试（install-destination.test.ts）**

```ts
import assert from "node:assert/strict";
import { test } from "node:test";

import { destinationOptions } from "./install-destination.ts";

test("windows offers the managed standard install next to a chosen directory", () => {
  const options = destinationOptions("windows");
  assert.equal(options.length, 2);
  assert.equal(options[0].id, "standard");
  assert.match(options[0].note ?? "", /系统设置/);
  assert.equal(options[1].id, "choose");
  assert.match(options[1].label, /选择/);
});

test("macos defaults to /Applications with a folder chooser", () => {
  const options = destinationOptions("macos");
  assert.equal(options[0].id, "standard");
  assert.match(options[0].label, /Applications/);
  assert.equal(options[1].id, "choose");
});

test("unknown platforms fall back to the windows-shaped choice", () => {
  assert.equal(destinationOptions("")[0].id, "standard");
});
```

- [x] **Step 2: 跑测试确认红**

Run: `cd codex/apps/codex-plus-manager && npm test -- install-destination`
Expected: FAIL（模块不存在）。

- [x] **Step 3: 实现**

`install-destination.ts`：

```ts
export interface DestinationOption {
  readonly id: "standard" | "choose";
  readonly label: string;
  readonly note: string | null;
}

/**
 * 首次安装的「安装位置」步骤（D-23）：Windows 的标准路线是 MSIX，装哪由系统管，
 * 选目录只能兑现到便携解压——两条都要摆出来，不默认替用户做取舍。
 */
export function destinationOptions(platform: string): readonly DestinationOption[] {
  if (platform === "macos") {
    return [
      { id: "standard", label: "默认位置（/Applications）", note: null },
      { id: "choose", label: "选择文件夹…", note: null },
    ];
  }
  return [
    {
      id: "standard",
      label: "标准安装（推荐）",
      note: "安装到 Windows 管理的位置；之后可在 系统设置 → 应用 中「移动」到其他盘",
    },
    { id: "choose", label: "选择安装目录…", note: "解压安装到所选目录，直接运行" },
  ];
}
```

`invoke.ts`：

```ts
export async function installOfficialApp(
  destination: string | null = null,
): Promise<LumioOfficialAppInstallStatus> {
  return runCommand<LumioOfficialAppInstallStatus>(LUMIO_COMMANDS.installOfficialApp, {
    destination,
  });
}
```

`HomeView.tsx`：新增 import 与状态——

```ts
import { open } from "@tauri-apps/plugin-dialog";
import { destinationOptions } from "../install-destination.ts";

const [destinationOpen, setDestinationOpen] = useState(false);

const chooseDirectory = async (): Promise<string | null> => {
  const picked = await open({ directory: true, multiple: false, title: "选择安装目录" });
  return typeof picked === "string" ? picked : null;
};

const installThenLaunch = (destination: string | null) => {
  setDestinationOpen(false);
  setLaunching(true);
  void (async () => {
    try {
      const started = await installOfficialApp(destination);
      /* …既有轮询逻辑不变… */
```

`onPrimaryClick`：

```ts
  const onPrimaryClick = () => {
    if (codexApp) {
      launch();
      return;
    }
    setDestinationOpen(true);
  };
```

渲染（仿 `paymentOpen` 的 modal 结构，插在其后）：

```tsx
      {destinationOpen ? (
        <div aria-modal="true" className="lumio-modal-backdrop" role="dialog">
          <div className="lumio-modal">
            <h3>选择安装位置</h3>
            {destinationOptions(state.bootstrap?.platform ?? "").map((option) => (
              <p key={option.id} className="lumio-settings-note">
                <button
                  className="lumio-button is-secondary"
                  onClick={() => {
                    if (option.id === "standard") {
                      installThenLaunch(null);
                      return;
                    }
                    void chooseDirectory().then((dir) => {
                      if (dir !== null) installThenLaunch(dir);
                    });
                  }}
                  type="button"
                >
                  {option.label}
                </button>
                {option.note === null ? null : <small>{option.note}</small>}
              </p>
            ))}
            <div className="lumio-modal-actions">
              <button
                className="lumio-button is-secondary"
                onClick={() => setDestinationOpen(false)}
                type="button"
              >
                取消
              </button>
            </div>
          </div>
        </div>
      ) : null}
```

- [x] **Step 4: 跑测试与校验确认绿**

Run: `npm test`、`npm run check`
Expected: 全部 PASS（含既有 install-progress / invoke 用例）。

- [x] **Step 5: Commit**（本会话不提交）

```bash
git add codex/apps/codex-plus-manager/src/lumio/
git commit -m "feat(codex): ask where to install the official app on first install"
```

---

### Task 5: 文档与收口

**Files:**
- Create: `.spec/decisions/0006-official-app-install-destination.md`
- Modify: `codex/docs/specs/2026-08-12-lumio-ux-interaction-design.md`（§5.5 安装 UX 增「安装位置」步骤）
- Modify: `docs/qa/2026-08-14-monorepo-qa-review.md`（D-23 状态 → ✅已实现+⏳待真机）
- Modify: `.spec/knowledge/features/lumio-account-and-home.md`（安装 bullet 补目的地/持久化/清包）

- [x] **Step 1: ADR-0006**（按 0005 的结构：Status/Context/Decision/Consequences）

核心决策内容：Windows 默认 MSIX 侧载，「选择目录」为并列一等选项（便携解压，Authenticode 钉选不降级）；macOS 目录选择直达；安装成功即删安装包；仅用户自选目录持久化到 `official-app-path.json` 并优先于自动探测。备选（默认便携 / 仅 macOS / 缓存跟随目标盘）与弃选理由一句话各记。

- [x] **Step 2: UX 规格 §5.5 增补**：主按钮点击 → 「选择安装位置」弹窗（两选项 + 说明文案）→ 开始安装；失败重试同样先经弹窗。原型变体暂不新增（记 known gap）。

- [x] **Step 3: QA D-23 批注**：已实现内容 + 待 Windows/macOS 真机验收（自选目录安装 → 重启管理器仍检测到 → 启动 → 聊天）。

- [x] **Step 4: 跑全部收口门槛**

```bash
cd codex && cargo fmt --all -- --check \
  && cargo test -p codex-plus-core --lib lumio \
  && cargo test -p codex-plus-core --test installers \
  && cargo test -p codex-plus-manager
cd apps/codex-plus-manager && npm run check && npm test
cd /Users/cui/Sites/lumio-codex && node .spec/tools/spec-lint.mjs
```

- [x] **Step 5: Commit**（本会话不提交）

```bash
git add .spec/ docs/ codex/docs/
git commit -m "docs(codex): record the install-destination decision and UX"
```

---

## Self-Review

- 覆盖检查：选目录（Task 1/2/3/4）、macOS 直达（Task 2/4）、清包（Task 2）、持久化+检测（Task 2）、文案与决策记录（Task 4/5）。「剩余空间预检 / 缓存迁移 / 手选持久化 / 原型变体」显式记为不做。
- 占位符扫描：无 TBD/TODO；Task 4 的轮询逻辑以「既有逻辑不变」引用原文件同段（实现者持上下文）。
- 类型一致性：`destination: Option<PathBuf>`（Rust IPC `Option<String>`）/ `destination: Option<&Path>`（PlanInput、live_install）/ TS `string | null`，各层命名统一为 `destination`。
