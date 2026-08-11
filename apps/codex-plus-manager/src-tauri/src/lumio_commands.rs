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
    let codex_app =
        codex_plus_core::app_paths::resolve_codex_app_dir(None).map(|path| LumioCodexAppPayload {
            version: codex_plus_core::app_paths::codex_app_version(&path),
            path: path.to_string_lossy().into_owned(),
            source: "automatic",
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
