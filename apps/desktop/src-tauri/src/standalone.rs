//! IPC commands for standalone mode and the pack manager. The webview names a mode, a pack
//! id or a variant, never a URL or a path: everything else comes from the installed manifest.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::Ordering::Relaxed;

use serde::Serialize;
use tauri::State;

use crate::local::{self, Job, LOCAL, Status};
use crate::packs::{self, PackError};
use crate::prefs::{self, Prefs};

fn local() -> Result<&'static local::Local, String> {
    LOCAL.get().ok_or_else(|| "not_available".to_string())
}

fn set_server(app: &tauri::AppHandle, prefs: &Mutex<Prefs>, url: &str) -> Result<(), String> {
    let mut p = prefs
        .lock()
        .map_err(|_| "internal state poisoned".to_string())?;
    p.server_url = url.to_string();
    prefs::save(app, &p)
}

/// Starts the local stack in the background and points the app at it when ready.
pub fn start_in_background(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let Ok(l) = local() else { return };
        if let Ok(url) = l.start().await {
            use tauri::Manager;
            let prefs = app.state::<Mutex<Prefs>>();
            if let Err(e) = set_server(&app, &prefs, &url) {
                tracing::warn!(error = %e, "could not save the local server address");
            }
        }
    });
}

#[tauri::command]
pub fn local_status() -> Result<Status, String> {
    Ok(LOCAL.get().map(local::Local::status).unwrap_or_default())
}

/// First-run choice: "server" (connect to a server, as before) or "local" (this PC only).
#[tauri::command]
pub fn local_choose_mode(
    app: tauri::AppHandle,
    prefs: State<'_, Mutex<Prefs>>,
    mode: String,
) -> Result<(), String> {
    if !matches!(mode.as_str(), "server" | "local") {
        return Err("invalid_mode".into());
    }
    if mode == "local" && local()?.paths.get().is_none() {
        return Err("not_available".into());
    }
    {
        let mut p = prefs
            .lock()
            .map_err(|_| "internal state poisoned".to_string())?;
        p.mode = Some(mode.clone());
        prefs::save(&app, &p)?;
    }
    if mode == "local" {
        start_in_background(app);
    }
    Ok(())
}

/// Retries a failed start.
#[tauri::command]
pub fn local_start(app: tauri::AppHandle) -> Result<(), String> {
    local()?;
    start_in_background(app);
    Ok(())
}

/// Creates the factory and its first administrator on this PC (once).
#[tauri::command]
pub async fn local_setup(
    session: State<'_, crate::Session>,
    factory_name: String,
    admin_name: String,
    email: String,
    password: String,
    locale: String,
) -> Result<(), String> {
    let text_ok = |s: &str| {
        let n = s.trim().chars().count();
        (1..=200).contains(&n) && !s.chars().any(char::is_control)
    };
    if !text_ok(&factory_name) || !text_ok(&admin_name) {
        return Err("validation_failed".into());
    }
    let email = email.trim().to_lowercase();
    if email.len() > 254
        || !email.contains('@')
        || email.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("validation_failed/email".into());
    }
    if password.chars().count() < 12 || password.len() > 1024 {
        return Err("validation_failed/password_too_short".into());
    }
    if !crate::validate::is_supported_language(&locale) {
        return Err("unsupported_language".into());
    }
    local()?
        .setup(
            &session.http,
            factory_name.trim(),
            admin_name.trim(),
            &email,
            &password,
            &locale,
        )
        .await
}

#[derive(Serialize)]
pub struct VariantView {
    variant: String,
    title_en: String,
    title_ja: String,
    min_ram_gb: u64,
    size: u64,
    installed: bool,
}

#[derive(Serialize)]
pub struct PackView {
    id: String,
    kind: String,
    version: String,
    title_en: String,
    title_ja: String,
    licence: String,
    licence_url: Option<String>,
    /// Bytes for this platform (default or installed variant for the AI model).
    size: u64,
    variants: Vec<VariantView>,
    /// "installed", "missing", "installing", "failed".
    status: &'static str,
    done: u64,
    total: u64,
    error: Option<String>,
}

#[derive(Serialize)]
pub struct PacksView {
    platform: &'static str,
    ram_bytes: u64,
    free_bytes: u64,
    recommended_variant: Option<String>,
    packs: Vec<PackView>,
}

fn job(id: &str) -> Option<Arc<Job>> {
    let l = LOCAL.get()?;
    l.jobs.lock().ok()?.get(id).cloned()
}

#[tauri::command]
pub async fn packs_status() -> Result<PacksView, String> {
    let l = local()?;
    let paths = l.paths.get().ok_or("not_available")?.clone();
    let m = l.manifest().map_err(|e| e.code().to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let root = paths.packs();
        let _ = std::fs::create_dir_all(&root);
        let installed = packs::Installed::load(&root);
        let (ram, free) = local::machine(&root);
        let recommended = m
            .pack("ai-model")
            .ok()
            .and_then(|p| p.recommended_variant(ram))
            .map(str::to_owned);
        let views = m
            .packs
            .iter()
            .map(|p| {
                let size_of = |v: Option<&str>| {
                    p.files_for(v)
                        .map(|fs| fs.iter().map(|f| f.size).sum())
                        .unwrap_or(0)
                };
                let variants: Vec<VariantView> = p
                    .variants
                    .iter()
                    .map(|v| VariantView {
                        variant: v.variant.clone(),
                        title_en: v.title_en.clone(),
                        title_ja: v.title_ja.clone(),
                        min_ram_gb: v.min_ram_gb,
                        size: size_of(Some(&v.variant)),
                        installed: packs::is_installed(&root, &installed, p, Some(&v.variant)),
                    })
                    .collect();
                let is_in = if p.variants.is_empty() {
                    packs::is_installed(&root, &installed, p, None)
                } else {
                    variants.iter().any(|v| v.installed)
                };
                let size = variants
                    .iter()
                    .find(|v| v.installed)
                    .or_else(|| {
                        variants
                            .iter()
                            .find(|v| Some(&v.variant) == recommended.as_ref())
                    })
                    .map_or_else(|| size_of(None), |v| v.size);
                let j = job(&p.id);
                let running = j.as_ref().is_some_and(|j| j.running.load(Relaxed));
                let error = j
                    .as_ref()
                    .and_then(|j| j.error.lock().ok().and_then(|e| e.clone()));
                let status = if running {
                    "installing"
                } else if is_in {
                    "installed"
                } else if error.is_some() {
                    "failed"
                } else {
                    "missing"
                };
                PackView {
                    id: p.id.clone(),
                    kind: p.kind.clone(),
                    version: p.version.clone(),
                    title_en: p.title_en.clone(),
                    title_ja: p.title_ja.clone(),
                    licence: p.licence.clone(),
                    licence_url: p.licence_url.clone(),
                    size,
                    variants,
                    status,
                    done: j.as_ref().map_or(0, |j| j.progress.done.load(Relaxed)),
                    total: j.as_ref().map_or(0, |j| j.progress.total.load(Relaxed)),
                    error,
                }
            })
            .collect();
        Ok(PacksView {
            platform: packs::PLATFORM,
            ram_bytes: ram,
            free_bytes: free,
            recommended_variant: recommended,
            packs: views,
        })
    })
    .await
    .map_err(|_| "pack_io".to_string())?
}

/// Installs one pack: core packs from the installer, large packs downloaded from the
/// manifest's upstream URL. Runs in the background; progress via `packs_status`.
#[tauri::command]
pub fn packs_install(pack_id: String, variant: Option<String>) -> Result<(), String> {
    let l = local()?;
    let m = l.manifest().map_err(|e| e.code().to_string())?;
    let pack = m.pack(&pack_id).map_err(|e| e.code().to_string())?.clone();
    if !pack.variants.is_empty()
        && !pack
            .variants
            .iter()
            .any(|v| Some(&v.variant) == variant.as_ref())
    {
        return Err(PackError::UnknownVariant.code().into());
    }
    let variant = if pack.variants.is_empty() {
        None
    } else {
        variant
    };
    let paths = l.paths.get().ok_or("not_available")?.clone();
    let root = paths.packs();
    std::fs::create_dir_all(&root).map_err(|_| "pack_io")?;
    if pack.kind == "large" {
        // Free space for what is still missing (resumed parts count as done) + 2 GB.
        let installed = packs::Installed::load(&root);
        let mut need = 0u64;
        for f in pack
            .files_for(variant.as_deref())
            .map_err(|e| e.code().to_string())?
        {
            if installed.has(&root, &pack.id, f) {
                continue;
            }
            let target =
                packs::resolve(&root, &pack.id, &f.path).map_err(|e| e.code().to_string())?;
            let part = std::fs::metadata(packs::part_path(&target))
                .map(|m| m.len())
                .unwrap_or(0);
            need += f.size.saturating_sub(part);
        }
        let (_, free) = local::machine(&root);
        if need > 0 && free < need + packs::DISK_HEADROOM {
            return Err(PackError::NoSpace.code().into());
        }
    }
    let job = {
        let mut jobs = l
            .jobs
            .lock()
            .map_err(|_| "internal state poisoned".to_string())?;
        if jobs.get(&pack_id).is_some_and(|j| j.running.load(Relaxed)) {
            return Err("pack_busy".into());
        }
        let j = Arc::new(Job::default());
        j.running.store(true, Relaxed);
        jobs.insert(pack_id.clone(), j.clone());
        j
    };
    tauri::async_runtime::spawn(async move {
        let result = if pack.kind == "core" {
            let (p, src, r) = (pack.clone(), paths.bundled_packs(), root.clone());
            tauri::async_runtime::spawn_blocking(move || packs::install_core(&p, &src, &r))
                .await
                .unwrap_or(Err(PackError::Io("join".into())))
        } else {
            match packs::download_client() {
                Ok(http) => {
                    packs::download_pack(&http, &pack, variant.as_deref(), &root, &job.progress)
                        .await
                }
                Err(e) => Err(PackError::Io(e)),
            }
        };
        match result {
            Ok(()) => {
                if let Some(v) = &variant {
                    l.set_ai_variant(v);
                }
                if let Ok(mut e) = job.error.lock() {
                    *e = None;
                }
                job.running.store(false, Relaxed);
                // Use the new pack right away if the local stack is running.
                if l.status().phase == "running" {
                    let _ = l.restart_services().await;
                }
            }
            Err(e) => {
                tracing::warn!(pack = %pack.id, error = ?e, "pack install failed");
                if let Ok(mut slot) = job.error.lock() {
                    *slot = Some(e.code().to_string());
                }
                job.running.store(false, Relaxed);
            }
        }
    });
    Ok(())
}

/// Stops a download; the partial file is kept so it resumes next time.
#[tauri::command]
pub fn packs_cancel(pack_id: String) -> Result<(), String> {
    if let Some(j) = job(&pack_id) {
        j.progress.cancel.store(true, Relaxed);
    }
    Ok(())
}
