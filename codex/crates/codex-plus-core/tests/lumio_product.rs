use codex_plus_core::lumio::product::{
    API_BASE_URL, BUNDLE_IDENTIFIER, DESKTOP_KEY_NAME, PRODUCT_NAME, SITE_BASE_URL, help_url,
    project_dirs,
};

#[test]
fn lumio_product_contract_is_stable() {
    assert_eq!(PRODUCT_NAME, "BestCodex");
    assert_eq!(BUNDLE_IDENTIFIER, "games.lumio.codex");
    assert_eq!(API_BASE_URL, "https://api.lumio.games/");
    assert_eq!(DESKTOP_KEY_NAME, "BestCodex Desktop");
    assert_eq!(SITE_BASE_URL, "https://bestcodex.app");
    assert_eq!(help_url(), "https://bestcodex.app/help");

    let dirs = project_dirs().expect("platform project directories");
    let data_local = dirs.data_local_dir().to_string_lossy();
    assert!(
        data_local.contains("BestCodex") || data_local.contains("bestcodex"),
        "project_dirs must follow PRODUCT_NAME: {data_local}"
    );
    assert!(
        !data_local.contains("Lumio Codex") && !data_local.contains("Lumio-Codex"),
        "state directory should no longer use Lumio Codex: {data_local}"
    );
    assert!(!data_local.contains("Codex++"));
}
