fn main() {
    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../codex-plus-manager/src-tauri/icons/icon.ico");
        resource.set("CompanyName", "Lumio");
        resource.set("FileDescription", "Lumio Codex Launcher");
        resource.set("InternalName", "lumio-codex-launcher");
        resource.set("OriginalFilename", "lumio-codex-launcher.exe");
        resource.set("ProductName", "Lumio Codex");
        resource.set_manifest(include_str!(
            "../codex-plus-manager/src-tauri/windows-app-manifest.xml"
        ));
        resource.compile().expect("compile launcher icon resource");
    }
}
