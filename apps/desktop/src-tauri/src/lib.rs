//! Native layer of the Chuo-Sogo Procure desktop client.
//!
//! The webview never sees the session token: it lives only in this process's memory
//! and every API call goes through `api_request`, which only reaches the configured
//! backend under `/api/v1/`. Signing out or closing the app discards the token.

mod prefs;
mod validate;

use std::sync::Mutex;

use serde::Serialize;
use serde_json::Value;
use tauri::{Manager, State};

use prefs::Prefs;

struct Session {
    token: Mutex<Option<String>>,
    http: reqwest::Client,
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
    base: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
    token: Option<String>,
) -> ApiResponse {
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut req = session
        .http
        .request(method, format!("{base}{path}"))
        .header("x-request-id", &request_id);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    if let Some(b) = body {
        req = req.json(&b);
    }
    match req.send().await {
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
    session: State<'_, Session>,
    url: String,
) -> Result<(), String> {
    let url = validate::server_url(&url)?;
    let mut p = lock(&prefs)?;
    if p.server_url != url {
        // A different backend must never receive the old backend's token.
        *lock(&session.token)? = None;
    }
    p.server_url = url;
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
    let mut res = send(
        &session,
        &base,
        reqwest::Method::POST,
        "/api/v1/auth/login",
        Some(serde_json::json!({ "email": email, "password": password })),
        None,
    )
    .await;
    if res.status == 200 {
        let token = res
            .body
            .as_object_mut()
            .and_then(|o| o.remove("token"))
            .and_then(|t| t.as_str().map(str::to_owned));
        *lock(&session.token)? = token;
    }
    Ok(res)
}

#[tauri::command]
async fn auth_logout(
    prefs: State<'_, Mutex<Prefs>>,
    session: State<'_, Session>,
) -> Result<(), String> {
    let base = lock(&prefs)?.server_url.clone();
    let token = lock(&session.token)?.take();
    if token.is_some() {
        // Best effort: the local token is already gone either way.
        send(
            &session,
            &base,
            reqwest::Method::POST,
            "/api/v1/auth/logout",
            None,
            token,
        )
        .await;
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
    validate::api_path(&path)?;
    let base = lock(&prefs)?.server_url.clone();
    let token = lock(&session.token)?.clone();
    let res = send(&session, &base, method, &path, body, token).await;
    if res.status == 401 {
        *lock(&session.token)? = None;
    }
    Ok(res)
}

/// Frontend errors are forwarded here so UI failures reach the same log as native ones.
#[tauri::command]
fn client_log(level: String, message: String, request_id: Option<String>) {
    let message: String = message.chars().take(2000).collect();
    let request_id = request_id.unwrap_or_default();
    match level.as_str() {
        "error" => tracing::error!(target: "procure_ui", %request_id, "{message}"),
        "warn" => tracing::warn!(target: "procure_ui", %request_id, "{message}"),
        _ => tracing::info!(target: "procure_ui", %request_id, "{message}"),
    }
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
