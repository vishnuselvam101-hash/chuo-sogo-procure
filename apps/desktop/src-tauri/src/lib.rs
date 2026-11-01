//! Native layer of the Chuo-Sogo Procure desktop client.
//!
//! The webview never sees the session token: it lives only in this process's memory,
//! bound to the server that issued it, and every API call goes through `api_request`,
//! which only reaches that server under its own API namespace (`/api/v1/`, or
//! `/api/admin/v1/` in the admin console build) and never follows redirects.
//! Signing out or closing the app discards the token.

#[cfg(feature = "admin")]
mod admin;
mod prefs;
mod validate;

use std::sync::Mutex;

use serde::Serialize;
use serde_json::Value;
use tauri::{Manager, State};

use prefs::Prefs;

/// A session token is only valid for the server that issued it.
struct BoundToken {
    server: String,
    token: String,
}

struct Session {
    token: Mutex<Option<BoundToken>>,
    /// Admin console: the step token between password and authenticator code.
    #[cfg(feature = "admin")]
    pending: Mutex<Option<admin::Pending>>,
    http: reqwest::Client,
}

impl Session {
    /// The token, but only if it was issued by `server`.
    fn token_for(&self, server: &str) -> Result<Option<String>, String> {
        Ok(lock(&self.token)?
            .as_ref()
            .filter(|b| b.server == server)
            .map(|b| b.token.clone()))
    }
}

#[derive(Serialize)]
pub struct ApiResponse {
    status: u16,
    body: Value,
    request_id: String,
}

fn network_error(request_id: String) -> ApiResponse {
    ApiResponse {
        status: 0,
        body: serde_json::json!({ "error": { "code": "network" } }),
        request_id,
    }
}

fn lock<T>(m: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>, String> {
    m.lock().map_err(|_| "internal state poisoned".to_string())
}

async fn send(
    session: &Session,
    url: reqwest::Url,
    method: reqwest::Method,
    body: Option<Value>,
    token: Option<String>,
) -> ApiResponse {
    let request_id = uuid::Uuid::new_v4().to_string();
    let upload = method == reqwest::Method::POST
        && (url.path().ends_with("/attachments") || url.path().ends_with("/media"));
    let mut req = session
        .http
        .request(method, url)
        .header("x-request-id", &request_id);
    if upload {
        req = req.timeout(TRANSFER_TIMEOUT);
    }
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    if let Some(b) = body {
        req = req.json(&b);
    }
    match req.send().await {
        // Redirects are disabled, so a 3xx arrives here and is reported, never followed.
        Ok(res) => {
            let status = res.status().as_u16();
            let body = res.json::<Value>().await.unwrap_or(Value::Null);
            ApiResponse {
                status,
                body,
                request_id,
            }
        }
        Err(e) => {
            tracing::warn!(%request_id, error = %e, "api request failed");
            network_error(request_id)
        }
    }
}

#[tauri::command]
fn prefs_get(prefs: State<'_, Mutex<Prefs>>) -> Result<Prefs, String> {
    Ok(lock(&prefs)?.clone())
}

#[tauri::command]
fn prefs_set_language(
    app: tauri::AppHandle,
    prefs: State<'_, Mutex<Prefs>>,
    language: Option<String>,
) -> Result<(), String> {
    if let Some(l) = &language
        && !validate::is_supported_language(l)
    {
        return Err("unsupported_language".into());
    }
    let mut p = lock(&prefs)?;
    p.language = language;
    prefs::save(&app, &p)
}

#[tauri::command]
fn prefs_set_server(
    app: tauri::AppHandle,
    prefs: State<'_, Mutex<Prefs>>,
    url: String,
) -> Result<(), String> {
    let url = validate::server_url(&url)?;
    let mut p = lock(&prefs)?;
    p.server_url = url;
    // No need to clear the token: it stays bound to the server that issued it and is
    // never sent anywhere else.
    prefs::save(&app, &p)
}

#[tauri::command]
async fn auth_login(
    prefs: State<'_, Mutex<Prefs>>,
    session: State<'_, Session>,
    email: String,
    password: String,
) -> Result<ApiResponse, String> {
    let base = lock(&prefs)?.server_url.clone();
    // Signing in again: revoke the previous session on its own server first (best effort).
    let previous = lock(&session.token)?.take();
    #[cfg(feature = "admin")]
    {
        *lock(&session.pending)? = None;
    }
    if let Some(prev) = previous
        && let Ok(url) = validate::api_url(&prev.server, &endpoint("auth/logout"))
    {
        send(&session, url, reqwest::Method::POST, None, Some(prev.token)).await;
    }
    let url = validate::api_url(&base, &endpoint("auth/login"))?;
    let body = serde_json::json!({ "email": email, "password": password });
    let mut res = send(&session, url, reqwest::Method::POST, Some(body), None).await;
    if res.status == 200 {
        let token = res
            .body
            .as_object_mut()
            .and_then(|o| o.remove("token"))
            .and_then(|t| t.as_str().map(str::to_owned));
        if let Some(token) = token {
            // Admin console: the password only unlocks the authenticator step.
            #[cfg(feature = "admin")]
            admin::after_password(&session, base, token, &res.body)?;
            #[cfg(not(feature = "admin"))]
            {
                *lock(&session.token)? = Some(BoundToken {
                    server: base,
                    token,
                });
            }
        }
    }
    Ok(res)
}

/// Trial server only: enters one of its listed trial accounts without a password (the server
/// refuses anywhere else). Same session handling as `auth_login`; not in the admin console.
#[cfg(not(feature = "admin"))]
#[tauri::command]
async fn auth_demo(
    prefs: State<'_, Mutex<Prefs>>,
    session: State<'_, Session>,
    email: String,
) -> Result<ApiResponse, String> {
    if email.len() > 254 {
        return Err("invalid email".into());
    }
    let base = lock(&prefs)?.server_url.clone();
    let previous = lock(&session.token)?.take();
    if let Some(prev) = previous
        && let Ok(url) = validate::api_url(&prev.server, &endpoint("auth/logout"))
    {
        send(&session, url, reqwest::Method::POST, None, Some(prev.token)).await;
    }
    let url = validate::api_url(&base, &endpoint("auth/demo-login"))?;
    let body = serde_json::json!({ "email": email });
    let mut res = send(&session, url, reqwest::Method::POST, Some(body), None).await;
    if res.status == 200
        && let Some(token) = res
            .body
            .as_object_mut()
            .and_then(|o| o.remove("token"))
            .and_then(|t| t.as_str().map(str::to_owned))
    {
        *lock(&session.token)? = Some(BoundToken {
            server: base,
            token,
        });
    }
    Ok(res)
}

/// `path` under this build's API namespace.
fn endpoint(path: &str) -> String {
    format!("{}{path}", validate::API_PREFIX)
}

#[tauri::command]
async fn auth_logout(session: State<'_, Session>) -> Result<(), String> {
    let bound = lock(&session.token)?.take();
    #[cfg(feature = "admin")]
    {
        *lock(&session.pending)? = None;
    }
    if let Some(b) = bound
        && let Ok(url) = validate::api_url(&b.server, &endpoint("auth/logout"))
    {
        // Best effort: the local token is already gone either way.
        send(&session, url, reqwest::Method::POST, None, Some(b.token)).await;
    }
    Ok(())
}

#[tauri::command]
async fn api_request(
    prefs: State<'_, Mutex<Prefs>>,
    session: State<'_, Session>,
    method: String,
    path: String,
    body: Option<Value>,
) -> Result<ApiResponse, String> {
    let method = validate::method(&method)?;
    let base = lock(&prefs)?.server_url.clone();
    let url = validate::api_url(&base, &path)?;
    let token = session.token_for(&base)?;
    let res = send(&session, url, method, body, token).await;
    if res.status == 401 {
        let mut t = lock(&session.token)?;
        if t.as_ref().is_some_and(|b| b.server == base) {
            *t = None;
        }
    }
    Ok(res)
}

/// Uploads and downloads of attachments (up to 10 MB) on slow links.
const TRANSFER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// Largest file (attachment or invoice PDF) the client will save (server limit is 10 MB).
const MAX_DOWNLOAD_BYTES: usize = 12 * 1024 * 1024;

/// Downloads one request attachment from the configured server and saves it into the
/// user's Downloads folder (never opened, never handed to the webview). Only the attachment
/// content endpoint can be reached, and only file types the server accepts can be written.
/// Returns the saved path in `body.saved_to`, or the server's error body.
#[cfg(not(feature = "admin"))]
#[tauri::command]
async fn api_download(
    app: tauri::AppHandle,
    prefs: State<'_, Mutex<Prefs>>,
    session: State<'_, Session>,
    org_id: String,
    attachment_id: String,
    file_name: String,
) -> Result<ApiResponse, String> {
    let org = uuid::Uuid::parse_str(&org_id).map_err(|_| "invalid_id".to_string())?;
    let attachment = uuid::Uuid::parse_str(&attachment_id).map_err(|_| "invalid_id".to_string())?;
    let name = validate::download_name(&file_name)
        .filter(|n| validate::is_attachment_type(n))
        .ok_or("invalid_file_name")?;
    let base = lock(&prefs)?.server_url.clone();
    let url = validate::api_url(
        &base,
        &format!("/api/v1/orgs/{org}/attachments/{attachment}/content"),
    )?;
    save_download(&app, &session, &base, url, &name).await
}

/// Largest listing photo the client will display.
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;

/// Fetches a listing photo and returns it as a `data:` URL for an `<img>` (the webview never
/// talks to the network itself). Only listing media content paths, only JPEG/PNG/WebP whose
/// bytes match the declared type, size-capped.
#[tauri::command]
async fn api_image(
    prefs: State<'_, Mutex<Prefs>>,
    session: State<'_, Session>,
    path: String,
) -> Result<ApiResponse, String> {
    if !validate::listing_media_path(&path) {
        return Err("invalid_path".into());
    }
    let base = lock(&prefs)?.server_url.clone();
    let url = validate::api_url(&base, &path)?;
    let token = session.token_for(&base)?;
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut req = session
        .http
        .get(url)
        .timeout(TRANSFER_TIMEOUT)
        .header("x-request-id", &request_id);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let mut res = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%request_id, error = %e, "image fetch failed");
            return Ok(network_error(request_id));
        }
    };
    let status = res.status().as_u16();
    if status != 200 {
        let body = res.json::<Value>().await.unwrap_or(Value::Null);
        return Ok(ApiResponse {
            status,
            body,
            request_id,
        });
    }
    let declared = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let mut bytes = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(|_| "network".to_string())? {
        if bytes.len() + chunk.len() > MAX_IMAGE_BYTES {
            return Err("download_too_large".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let mime = validate::image_type(&declared, &bytes).ok_or("not_an_image")?;
    Ok(ApiResponse {
        status,
        body: serde_json::json!({ "data_url": format!("data:{mime};base64,{}", base64_encode(&bytes)) }),
        request_id,
    })
}

/// Standard base64 (no external crate needed for this one use).
fn base64_encode(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if i <= c.len() {
                out.push(char::from(T[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Saves an issued invoice or credit note PDF (`<number>.pdf`) into the Downloads folder.
/// Business app: the customer's own documents; admin console: any organisation's.
#[tauri::command]
async fn invoice_download(
    app: tauri::AppHandle,
    prefs: State<'_, Mutex<Prefs>>,
    session: State<'_, Session>,
    org_id: String,
    invoice_id: String,
    number: String,
) -> Result<ApiResponse, String> {
    let org = uuid::Uuid::parse_str(&org_id).map_err(|_| "invalid_id".to_string())?;
    let invoice = uuid::Uuid::parse_str(&invoice_id).map_err(|_| "invalid_id".to_string())?;
    let name = validate::invoice_file_name(&number).ok_or("invalid_file_name")?;
    let base = lock(&prefs)?.server_url.clone();
    let url = validate::api_url(
        &base,
        &endpoint(&format!("orgs/{org}/billing/invoices/{invoice}/pdf")),
    )?;
    save_download(&app, &session, &base, url, &name).await
}

/// GETs `url` with the session for `base` (size-capped) and writes it to the Downloads
/// folder as `name`, never overwriting.
async fn save_download(
    app: &tauri::AppHandle,
    session: &Session,
    base: &str,
    url: reqwest::Url,
    name: &str,
) -> Result<ApiResponse, String> {
    let token = session.token_for(base)?;
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut req = session
        .http
        .get(url)
        .timeout(TRANSFER_TIMEOUT)
        .header("x-request-id", &request_id);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let mut res = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%request_id, error = %e, "download failed");
            return Ok(network_error(request_id));
        }
    };
    let status = res.status().as_u16();
    if status != 200 {
        if status == 401 {
            let mut t = lock(&session.token)?;
            if t.as_ref().is_some_and(|b| b.server == base) {
                *t = None;
            }
        }
        let body = res.json::<Value>().await.unwrap_or(Value::Null);
        return Ok(ApiResponse {
            status,
            body,
            request_id,
        });
    }
    // Stream with a hard cap instead of trusting Content-Length.
    let mut bytes = Vec::new();
    loop {
        match res.chunk().await {
            Ok(Some(chunk)) => {
                if bytes.len() + chunk.len() > MAX_DOWNLOAD_BYTES {
                    return Err("download_too_large".into());
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(e) => {
                tracing::warn!(%request_id, error = %e, "download interrupted");
                return Ok(network_error(request_id));
            }
        }
    }
    // The platform's Downloads folder; on Linux without XDG user dirs, ~/Downloads.
    let dir = app
        .path()
        .download_dir()
        .or_else(|_| app.path().home_dir().map(|h| h.join("Downloads")))
        .map_err(|_| "no_download_folder".to_string())?;
    std::fs::create_dir_all(&dir).map_err(|_| "no_download_folder".to_string())?;
    let saved = save_unique(&dir, name, &bytes)?;
    tracing::info!(%request_id, "file saved");
    Ok(ApiResponse {
        status,
        body: serde_json::json!({ "saved_to": saved.to_string_lossy() }),
        request_id,
    })
}

/// Writes `bytes` as `name` in `dir`, never overwriting: "a.pdf", "a (1).pdf", ...
fn save_unique(
    dir: &std::path::Path,
    name: &str,
    bytes: &[u8],
) -> Result<std::path::PathBuf, String> {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for n in 0..1000 {
        let candidate = if n == 0 {
            dir.join(name)
        } else {
            dir.join(format!("{stem} ({n}){ext}"))
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut f) => {
                if std::io::Write::write_all(&mut f, bytes).is_err() {
                    drop(f);
                    // Never leave a partial file behind.
                    let _ = std::fs::remove_file(&candidate);
                    return Err("save_failed".into());
                }
                return Ok(candidate);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err("save_failed".into()),
        }
    }
    Err("save_failed".into())
}

/// Frontend errors are forwarded here so UI failures reach the same log as native ones.
#[tauri::command]
fn client_log(level: String, message: String, request_id: Option<String>) -> Result<(), String> {
    let message = validate::log_text(&message);
    let request_id = request_id
        .filter(|r| uuid::Uuid::parse_str(r).is_ok())
        .unwrap_or_default();
    match level.as_str() {
        "error" => tracing::error!(target: "procure_ui", %request_id, "{message}"),
        "warn" => tracing::warn!(target: "procure_ui", %request_id, "{message}"),
        "info" => tracing::info!(target: "procure_ui", %request_id, "{message}"),
        _ => return Err("invalid_level".into()),
    }
    Ok(())
}

/// Business app commands. Attachments are only downloaded here.
#[cfg(not(feature = "admin"))]
fn handlers() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        prefs_get,
        prefs_set_language,
        prefs_set_server,
        auth_login,
        auth_demo,
        auth_logout,
        api_request,
        api_download,
        invoice_download,
        api_image,
        client_log
    ]
}

/// Admin console commands: the authenticator sign-in steps; the only file it writes is an
/// issued invoice PDF.
#[cfg(feature = "admin")]
fn handlers() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        prefs_get,
        prefs_set_language,
        prefs_set_server,
        auth_login,
        auth_logout,
        api_request,
        admin::auth_enroll,
        admin::auth_code,
        invoice_download,
        api_image,
        client_log
    ]
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("procure_desktop_lib=info,procure_ui=info")
            }),
        )
        .init();

    let http = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(concat!("ChuoSogoProcure/", env!("CARGO_PKG_VERSION")))
        // Never follow redirects: they could carry the token or a password elsewhere.
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cannot initialise HTTP client: {e}");
            std::process::exit(1);
        }
    };

    let result = tauri::Builder::default()
        .manage(Session {
            token: Mutex::new(None),
            #[cfg(feature = "admin")]
            pending: Mutex::new(None),
            http,
        })
        .setup(|app| {
            let loaded = prefs::load(app.handle());
            app.manage(Mutex::new(loaded));
            Ok(())
        })
        .invoke_handler(handlers())
        .run(tauri::generate_context!());
    if let Err(e) = result {
        eprintln!("application error: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{base64_encode, save_unique};

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn saving_never_overwrites() {
        // Unique random name per run; test-only scratch directory.
        // nosemgrep: rust.lang.security.temp-dir.temp-dir
        let dir = std::env::temp_dir().join(format!("procure-dl-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = save_unique(&dir, "見積.pdf", b"1").unwrap();
        let b = save_unique(&dir, "見積.pdf", b"2").unwrap();
        assert_eq!(a.file_name().unwrap(), "見積.pdf");
        assert_eq!(b.file_name().unwrap(), "見積 (1).pdf");
        assert_eq!(std::fs::read(&a).unwrap(), b"1");
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod capability_tests {
    /// The business window can never call the admin sign-in steps, and the admin console can
    /// never download files: each build's capability grants only its own commands.
    #[test]
    fn each_app_is_granted_only_its_own_commands() {
        let business = include_str!("../capabilities/default.json");
        let admin = include_str!("../capabilities/admin.json");
        for cmd in ["allow-auth-enroll", "allow-auth-code"] {
            assert!(!business.contains(cmd), "business grants {cmd}");
            assert!(admin.contains(cmd), "admin lacks {cmd}");
        }
        assert!(business.contains("allow-api-download"));
        assert!(!admin.contains("allow-api-download"));
        assert!(
            business.contains("allow-invoice-download") && admin.contains("allow-invoice-download")
        );
        let config = include_str!("../tauri.admin.conf.json");
        assert!(config.contains(r#""capabilities": ["admin"]"#));
        assert!(config.contains(r#""identifier": "net.chuo-sogo.procure.admin""#));
    }
}
