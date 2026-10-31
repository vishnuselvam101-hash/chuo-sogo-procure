//! Native layer of the Chuo-Sogo Procure desktop client.
//!
//! The webview never sees the session token: it lives only in this process's memory,
//! bound to the server that issued it, and every API call goes through `api_request`,
//! which only reaches that server under `/api/v1/` and never follows redirects.
//! Signing out or closing the app discards the token.

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
    let mut req = session
        .http
        .request(method, url)
        .header("x-request-id", &request_id);
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
    if let Some(prev) = previous
        && let Ok(url) = validate::api_url(&prev.server, "/api/v1/auth/logout")
    {
        send(&session, url, reqwest::Method::POST, None, Some(prev.token)).await;
    }
    let url = validate::api_url(&base, "/api/v1/auth/login")?;
    let body = serde_json::json!({ "email": email, "password": password });
    let mut res = send(&session, url, reqwest::Method::POST, Some(body), None).await;
    if res.status == 200 {
        let token = res
            .body
            .as_object_mut()
            .and_then(|o| o.remove("token"))
            .and_then(|t| t.as_str().map(str::to_owned));
        if let Some(token) = token {
            *lock(&session.token)? = Some(BoundToken {
                server: base,
                token,
            });
        }
    }
    Ok(res)
}

#[tauri::command]
async fn auth_logout(session: State<'_, Session>) -> Result<(), String> {
    let bound = lock(&session.token)?.take();
    if let Some(b) = bound
        && let Ok(url) = validate::api_url(&b.server, "/api/v1/auth/logout")
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
            http,
        })
        .setup(|app| {
            let loaded = prefs::load(app.handle());
            app.manage(Mutex::new(loaded));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            prefs_get,
            prefs_set_language,
            prefs_set_server,
            auth_login,
            auth_logout,
            api_request,
            client_log
        ])
        .run(tauri::generate_context!());
    if let Err(e) = result {
        eprintln!("application error: {e}");
        std::process::exit(1);
    }
}
