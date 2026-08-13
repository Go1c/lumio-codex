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
