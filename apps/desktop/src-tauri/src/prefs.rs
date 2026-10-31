//! Local, non-sensitive preferences (language, server address) in the app config dir.
//! Never holds credentials or procurement data.

use serde::{Deserialize, Serialize};
use tauri::Manager;

#[derive(Clone, Serialize, Deserialize)]
pub struct Prefs {
    /// User's explicit choice; `None` means "use the organisation default".
    pub language: Option<String>,
    pub server_url: String,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            language: None,
            server_url: "http://127.0.0.1:8088".into(),
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
