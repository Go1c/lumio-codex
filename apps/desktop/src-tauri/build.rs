fn main() {
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-env-changed=PROFILE");
    println!("cargo:rerun-if-env-changed=TARGET");

    if std::env::var_os("PROFILE").is_some_and(|profile| profile == "release") {
        let target = std::env::var("TARGET").expect("release target triple");
        let sidecar = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("desktop manifest directory"),
        )
        .join("binaries")
        .join(format!("fns-agent-{target}"));
        if !sidecar.is_file() {
            panic!(
                "release sidecar missing at {}; run scripts/stage-macos-arm64-sidecar.sh first",
                sidecar.display()
            );
        }
        println!("cargo:rerun-if-changed={}", sidecar.display());

        let resource_root = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("desktop manifest directory"),
        )
        .join("resources")
        .join("remote")
        .join("linux-x86_64");
        for artifact in ["fns-server", "fns-agent"] {
            let path = resource_root.join(artifact);
            if !path.is_file() {
                panic!(
                    "release remote artifact missing at {}; run scripts/stage-remote-linux-x86_64-artifacts.sh first",
                    path.display()
                );
            }
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    if std::env::var_os("PROFILE").is_some_and(|profile| profile != "release")
        && std::env::var_os("FNS_TAURI_DEV_BUILD_CHILD").is_none()
    {
        let status =
            std::process::Command::new(std::env::current_exe().expect("build script path"))
                .env("FNS_TAURI_DEV_BUILD_CHILD", "1")
                .env(
                    "TAURI_CONFIG",
                    r#"{"bundle":{"externalBin":null,"resources":null}}"#,
                )
                .status()
                .expect("run development Tauri build helper");
        if !status.success() {
            panic!("development Tauri build helper failed");
        }
        return;
    }
    tauri_build::build()
}
