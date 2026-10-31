fn main() {
    // Every IPC command must be listed here and granted in capabilities/default.json;
    // anything else is rejected by the runtime.
    let manifest = tauri_build::AppManifest::new().commands(&[
        "prefs_get",
        "prefs_set_language",
        "prefs_set_server",
        "auth_login",
        "auth_logout",
        "api_request",
        "client_log",
    ]);
    if let Err(e) = tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest)) {
        eprintln!("tauri build script failed: {e}");
        std::process::exit(1);
    }
}
