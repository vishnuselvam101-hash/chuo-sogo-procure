//! Admin console sign-in (feature `admin`): password → (first time: set up an authenticator)
//! → 6-digit code → admin session.
//!
//! The short-lived step token from the password check stays in this process like the session
//! token; the webview only learns which step comes next.

use serde_json::{Value, json};
use tauri::State;

use crate::{ApiResponse, BoundToken, Session, lock, send, validate};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Mfa,
    Enroll,
}

pub struct Pending {
    server: String,
    token: String,
    step: Step,
}

/// After a successful password check: keep the step token, tell the webview the step.
pub fn after_password(
    session: &Session,
    server: String,
    token: String,
    body: &Value,
) -> Result<(), String> {
    let step = match body.get("step").and_then(Value::as_str) {
        Some("mfa") => Step::Mfa,
        Some("enroll") => Step::Enroll,
        _ => return Err("unexpected_response".into()),
    };
    *lock(&session.pending)? = Some(Pending {
        server,
        token,
        step,
    });
    Ok(())
}

fn pending(session: &Session, step: Option<Step>) -> Result<(String, String, Step), String> {
    lock(&session.pending)?
        .as_ref()
        .filter(|p| step.is_none_or(|s| s == p.step))
        .map(|p| (p.server.clone(), p.token.clone(), p.step))
        .ok_or_else(|| "no_sign_in_step".to_string())
}

/// First sign-in: a new authenticator secret (shown once, active only after a code confirms it).
#[tauri::command]
pub async fn auth_enroll(session: State<'_, Session>) -> Result<ApiResponse, String> {
    let (server, token, _) = pending(&session, Some(Step::Enroll))?;
    let url = validate::api_url(&server, "/api/admin/v1/auth/totp/enroll")?;
    Ok(send(
        &session,
        url,
        reqwest::Method::POST,
        Some(json!({ "token": token })),
        None,
    )
    .await)
}

/// The authenticator code: completes sign-in (or confirms a new authenticator).
#[tauri::command]
pub async fn auth_code(session: State<'_, Session>, code: String) -> Result<ApiResponse, String> {
    let code = validate::totp_code(&code)?;
    let (server, token, step) = pending(&session, None)?;
    let path = match step {
        Step::Mfa => "/api/admin/v1/auth/mfa",
        Step::Enroll => "/api/admin/v1/auth/totp/confirm",
    };
    let url = validate::api_url(&server, path)?;
    let body = json!({ "token": token, "code": code });
    let mut res = send(&session, url, reqwest::Method::POST, Some(body), None).await;
    if res.status == 200 {
        let session_token = res
            .body
            .as_object_mut()
            .and_then(|o| o.remove("token"))
            .and_then(|t| t.as_str().map(str::to_owned));
        if let Some(t) = session_token {
            *lock(&session.pending)? = None;
            *lock(&session.token)? = Some(BoundToken { server, token: t });
        }
    } else if res.status == 401 {
        // A wrong code answers 401 too; the step token stays until the server rejects it
        // as expired, which the user sees as "sign in again".
        tracing::info!("authenticator code rejected");
    }
    Ok(res)
}
