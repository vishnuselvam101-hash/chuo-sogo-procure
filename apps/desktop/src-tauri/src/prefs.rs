//! Local, non-sensitive preferences (language, server address) in the app config dir.
//! Never holds credentials or procurement data.

use serde::{Deserialize, Serialize};
use tauri::Manager;

#[derive(Clone, Serialize, Deserialize)]
pub struct Prefs {
    /// User's explicit choice; `None` means "use the organisation default".
    pub language: Option<String>,
    pub server_url: String,
    /// Reading comfort: "normal", "large" or "xlarge". Display only.
    #[serde(default = "default_text_size")]
    pub text_size: String,
    /// Stronger colours and outlines. Display only.
    #[serde(default)]
    pub high_contrast: bool,
    /// "server" (connect to a server) or "local" (this PC only). `None` only on a fresh
    /// installation that carries the standalone bundle: the app then asks. Prefs files from
    /// before this setting mean "server", so existing installations behave as before.
    #[serde(default = "server_mode")]
    pub mode: Option<String>,
}

fn server_mode() -> Option<String> {
    Some("server".into())
}

/// Text sizes the UI offers (Settings → Display).
pub const TEXT_SIZES: [&str; 3] = ["normal", "large", "xlarge"];

fn default_text_size() -> String {
    TEXT_SIZES[0].into()
}

/// Server a fresh installation connects to: set at build time with `PROCURE_DEFAULT_SERVER`
/// (e.g. the demo server for trial installers); a local development server otherwise. Users
/// can change it on the sign-in screen.
const DEFAULT_SERVER: &str = match option_env!("PROCURE_DEFAULT_SERVER") {
    Some(url) if !url.is_empty() => url,
    _ => "http://127.0.0.1:8088",
};

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            language: None,
            server_url: DEFAULT_SERVER.into(),
            text_size: default_text_size(),
            high_contrast: false,
            mode: None,
        }
    }
}

fn path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|d| d.join("prefs.json"))
}

pub fn load(app: &tauri::AppHandle) -> Prefs {
    let Some(p) = path(app) else {
        return Prefs::default();
    };
    let parsed = std::fs::read(&p)
        .ok()
        .and_then(|b| serde_json::from_slice::<Prefs>(&b).ok());
    match parsed {
        // Re-validate on load: the file is outside our control.
        Some(mut prefs) => {
            if prefs
                .language
                .as_deref()
                .is_some_and(|l| !crate::validate::is_supported_language(l))
            {
                prefs.language = None;
            }
            if !TEXT_SIZES.contains(&prefs.text_size.as_str()) {
                prefs.text_size = default_text_size();
            }
            if !matches!(prefs.mode.as_deref(), Some("server" | "local")) {
                prefs.mode = server_mode();
            }
            match crate::validate::server_url(&prefs.server_url) {
                Ok(u) => prefs.server_url = u,
                Err(_) => prefs.server_url = Prefs::default().server_url,
            }
            prefs
        }
        None => Prefs::default(),
    }
}

pub fn save(app: &tauri::AppHandle, prefs: &Prefs) -> Result<(), String> {
    let p = path(app).ok_or("no_config_dir")?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(prefs).map_err(|e| e.to_string())?;
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::Prefs;

    #[test]
    fn older_prefs_files_still_load_with_display_defaults() {
        let p: Prefs =
            serde_json::from_str(r#"{"language":"ja","server_url":"https://a.test"}"#).unwrap();
        assert_eq!(p.text_size, "normal");
        assert!(!p.high_contrast);
        assert_eq!(p.mode.as_deref(), Some("server"));
    }
}
