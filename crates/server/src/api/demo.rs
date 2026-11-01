//! Password-free entry for the public trial server (`PROCURE_DEMO_LOGIN`, a list of trial
//! account emails). Off unless configured: real deployments never have it. Only the listed
//! accounts can be entered, never one that holds the platform super-admin role, and every
//! entry is rate-limited and logged. The apps show the list as role buttons.

use axum::Json;
use axum::extract::State;
use procure_domain::rbac::{OrgKind, Role};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::auth::{LoginResponse, start_session};
use crate::AppState;
use crate::db;
use crate::error::{ApiError, ApiJson, ApiResult, ErrorBody};
use crate::principal::ClientIp;
use crate::ratelimit;

#[derive(Serialize, utoipa::ToSchema)]
pub struct DemoAccount {
    pub email: String,
    pub display_name: String,
    /// Role code in the account's organisation (translated by the apps).
    pub role: String,
    pub org_name: String,
    /// `platform`, `customer`, `supplier` or `consumer`: which app the account belongs in.
    pub org_kind: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct DemoLoginRequest {
    pub email: String,
}

/// id, display name, then (org name, org kind, role) of the first active membership.
type AccountRow = (Uuid, String, Option<(String, String, String)>);

async fn account(state: &AppState, email: &str) -> ApiResult<Option<AccountRow>> {
    let mut tx = db::auth_tx(&state.pool).await?;
    let user: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, display_name FROM users WHERE email = $1 AND status = 'active'")
            .bind(email)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((id, name)) = user else {
        return Ok(None);
    };
    db::set_user(&mut tx, id).await?;
    let membership: Option<(String, String, String)> = sqlx::query_as(
        "SELECT o.name, o.kind, m.role FROM memberships m JOIN organisations o ON o.id = m.org_id
         WHERE m.user_id = $1 AND m.status = 'active' AND o.status = 'active'
         ORDER BY o.kind DESC, o.name LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some((id, name, membership)))
}

fn is_super_admin(m: &Option<(String, String, String)>) -> bool {
    m.as_ref()
        .is_some_and(|(_, _, role)| Role::from_code(role) == Some(Role::PlatformSuperAdmin))
}

#[utoipa::path(
    operation_id = "demo_accounts",
    get,
    path = "/api/v1/demo/accounts",
    tag = "demo",
    responses(
        (status = 200, description = "Trial accounts that can be entered without a password", body = [DemoAccount]),
        (status = 404, description = "Not a trial server", body = ErrorBody),
    )
)]
pub async fn accounts(State(state): State<AppState>) -> ApiResult<Json<Vec<DemoAccount>>> {
    if state.demo_accounts.is_empty() {
        return Err(ApiError::NotFound);
    }
    let mut out = Vec::new();
    for email in &state.demo_accounts {
        let Some((_, name, membership)) = account(&state, email).await? else {
            continue;
        };
        if is_super_admin(&membership) {
            continue;
        }
        let Some((org_name, kind, role)) = membership else {
            continue;
        };
        if OrgKind::from_code(&kind).is_none() {
            continue;
        }
        out.push(DemoAccount {
            email: email.clone(),
            display_name: name,
            role,
            org_name,
            org_kind: kind,
        });
    }
    Ok(Json(out))
}

#[utoipa::path(
    operation_id = "demo_login",
    post,
    path = "/api/v1/auth/demo-login",
    tag = "demo",
    request_body = DemoLoginRequest,
    responses(
        (status = 200, description = "Signed in as the trial account", body = LoginResponse),
        (status = 401, description = "Not a trial account", body = ErrorBody),
        (status = 404, description = "Not a trial server", body = ErrorBody),
        (status = 429, description = "Rate limited", body = ErrorBody),
    )
)]
pub async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(req): ApiJson<DemoLoginRequest>,
) -> ApiResult<Json<LoginResponse>> {
    if state.demo_accounts.is_empty() {
        return Err(ApiError::NotFound);
    }
    if !state.limiter.check(
        &format!("login-ip:{}", ratelimit::ip_key(&ip)),
        &ratelimit::LOGIN_PER_IP,
    ) {
        return Err(ApiError::RateLimited);
    }
    let email = req.email.trim().to_lowercase();
    if !state.demo_accounts.contains(&email) {
        return Err(ApiError::InvalidCredentials);
    }
    let Some((user_id, _, membership)) = account(&state, &email).await? else {
        return Err(ApiError::InvalidCredentials);
    };
    // Never the platform super-admin, even if someone lists it by mistake.
    if is_super_admin(&membership) {
        return Err(ApiError::InvalidCredentials);
    }
    tracing::info!(%user_id, %ip, "trial account entered without password");
    Ok(Json(start_session(&state, &ip, user_id).await?))
}
