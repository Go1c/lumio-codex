use std::fs;
use std::path::{Path, PathBuf};

// 占位内容必须小于运行时「真制品」下限（1024 字节），保证占位永远被
// is_real_artifact / is_real_sidecar 拒绝；真产物由 scripts/sync-components/stage.mjs 覆盖。
fn ensure_placeholder(path: &Path, contents: &[u8]) {
    if path.exists() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, contents);
}

fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let triple = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    let ext = if triple.contains("windows") {
        ".exe"
    } else {
        ""
    };
    ensure_placeholder(
        &manifest
            .join("binaries")
            .join(format!("fns-agent-{triple}{ext}")),
        b"placeholder\n",
    );
    // CI host triples required by sidecar_placeholders_cover_ci_host_triples
    // (and by a fresh checkout). TARGET above covers linux CI later.
    for name in [
        "fns-agent-aarch64-apple-darwin",
        "fns-agent-x86_64-apple-darwin",
        "fns-agent-x86_64-pc-windows-msvc.exe",
    ] {
        ensure_placeholder(&manifest.join("binaries").join(name), b"placeholder\n");
    }
    let remote = manifest.join("resources/remote/linux-x86_64");
    ensure_placeholder(&remote.join("fns-server"), b"placeholder\n");
    ensure_placeholder(&remote.join("fns-agent"), b"placeholder\n");
    ensure_placeholder(&remote.join("release-provenance.json"), b"{}");

    let windows = tauri_build::WindowsAttributes::new()
        .app_manifest(include_str!("windows-app-manifest.xml"));
    let attrs = tauri_build::Attributes::new().windows_attributes(windows);
    tauri_build::try_build(attrs).expect("failed to run Tauri build script");
}
