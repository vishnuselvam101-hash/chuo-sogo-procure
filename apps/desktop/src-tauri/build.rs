fn main() {
    // Preset server address for trial installers. Mobile builds run cargo from Xcode/Gradle,
    // which do not pass the workflow's environment through, so the address can also come
    // from `default-server.txt` next to this file (written by the build workflow).
    println!("cargo:rerun-if-env-changed=PROCURE_DEFAULT_SERVER");
    if std::path::Path::new("default-server.txt").exists() {
        println!("cargo:rerun-if-changed=default-server.txt");
    }
    let preset = std::env::var("PROCURE_DEFAULT_SERVER")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::fs::read_to_string("default-server.txt").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(url) = preset {
        println!("cargo:rustc-env=PROCURE_DEFAULT_SERVER={url}");
    }

    // Every IPC command must be listed here and granted in a capability;
    // anything else is rejected by the runtime.
    let manifest = tauri_build::AppManifest::new().commands(&[
        "prefs_get",
        "prefs_set_language",
        "prefs_set_server",
        "auth_login",
        "auth_logout",
        "api_request",
        "api_download",
        "invoice_download",
        "api_image",
        "client_log",
        // Admin console only (feature `admin`, granted in capabilities/admin.json).
        "auth_enroll",
        "auth_code",
    ]);
    if let Err(e) = tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest)) {
        eprintln!("tauri build script failed: {e}");
        std::process::exit(1);
    }
}
