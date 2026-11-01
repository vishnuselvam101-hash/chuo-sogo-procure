//! Login, logout, current user, invitations, activation and password reset.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use procure_domain::rbac::{self, OrgKind, Role};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::audit::{self, Event};
use crate::db;
use crate::error::{ApiError, ApiJson, ApiPath, ApiResult, ErrorBody};
use crate::mailer::Mail;
use crate::principal::{ClientIp, Membership, Principal, load_memberships};
use crate::ratelimit;
use crate::security;

pub const SESSION_TTL: Duration = Duration::hours(12);
pub const MAX_FAILED_LOGINS: i32 = 10;
pub const LOCKOUT: Duration = Duration::minutes(15);
pub const RESET_TOKEN_TTL: Duration = Duration::hours(1);
/// A new reset mail is not issued while a recent one is still usable, so repeated
/// requests cannot keep invalidating the link the real user is about to click.
pub const RESET_COOLDOWN: Duration = Duration::minutes(5);
pub const ACTIVATION_TOKEN_TTL: Duration = Duration::days(7);

#[derive(Deserialize, utoipa::ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct UserView {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub locale: Option<String>,
    pub account_kind: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct MembershipView {
    pub org_id: Uuid,
    pub org_name: String,
    pub org_kind: &'static str,
    pub default_locale: String,
    pub timezone: String,
    pub role: &'static str,
    pub permissions: Vec<&'static str>,
    /// `member` of the organisation, or internal staff `assigned` to serve it.
    pub access: &'static str,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct InvitationView {
    pub org_id: Uuid,
    pub org_name: String,
    pub role: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct MeResponse {
    pub user: UserView,
    pub memberships: Vec<MembershipView>,
    pub invitations: Vec<InvitationView>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct LoginResponse {
    pub token: String,
    pub expires_at: DateTime<Utc>,
    #[serde(flatten)]
    pub me: MeResponse,
}

type UserRow = (Uuid, String, String, Option<String>, String);

async fn me_view(
    tx: &mut db::Tx,
    user_id: Uuid,
    memberships: &[Membership],
) -> ApiResult<MeResponse> {
    let (id, email, display_name, locale, account_kind): UserRow = sqlx::query_as(
        "SELECT id, email::text, display_name, locale, account_kind FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    let mut views = Vec::with_capacity(memberships.len());
    for m in memberships {
        let (org_name, default_locale, timezone): (String, String, String) = sqlx::query_as(
            "SELECT name, default_locale, timezone FROM organisations WHERE id = $1",
        )
        .bind(m.org_id)
        .fetch_one(&mut **tx)
        .await?;
        views.push(MembershipView {
            org_id: m.org_id,
            org_name,
            org_kind: m.org_kind.code(),
            default_locale,
            timezone,
            role: m.role.code(),
            permissions: rbac::home_permissions(m.role)
                .iter()
                .map(|p| p.code())
                .collect(),
            access: "member",
        });
    }
    // Internal staff also work in the customer organisations they are assigned to.
    if let Some(role) = memberships
        .iter()
        .find(|m| m.org_kind == OrgKind::Platform)
        .map(|m| m.role)
    {
        let assigned: Vec<(Uuid, String, String, String)> = sqlx::query_as(
            "SELECT o.id, o.name, o.default_locale, o.timezone
             FROM service_assignments a JOIN organisations o ON o.id = a.org_id
             WHERE a.user_id = $1 AND a.revoked_at IS NULL AND o.status = 'active'
             ORDER BY o.name, o.id",
        )
        .bind(user_id)
        .fetch_all(&mut **tx)
        .await?;
        for (org_id, org_name, default_locale, timezone) in assigned {
            views.push(MembershipView {
                org_id,
                org_name,
                org_kind: OrgKind::Customer.code(),
                default_locale,
                timezone,
                role: role.code(),
                permissions: rbac::service_permissions(role)
                    .iter()
                    .map(|p| p.code())
                    .collect(),
                access: "assigned",
            });
        }
    }
    let invitations = sqlx::query_as::<_, InvitationView>(
        "SELECT m.org_id, o.name AS org_name, m.role FROM memberships m JOIN organisations o ON o.id = m.org_id
         WHERE m.user_id = $1 AND m.status = 'invited' AND o.status = 'active' ORDER BY m.created_at",
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(MeResponse {
        user: UserView {
            id,
            email,
            display_name,
            locale,
            account_kind,
        },
        memberships: views,
        invitations,
    })
}

/// id, password_hash, status, locked_until
type LoginRow = (Uuid, Option<String>, String, Option<DateTime<Utc>>);

/// Password step shared by every login (apps and admin console): rate limits, uniform
/// handling of unknown/inactive/locked accounts, lockout after repeated failures.
pub async fn check_password(
    state: &AppState,
    ip: &str,
    email_raw: &str,
    password: String,
) -> ApiResult<Uuid> {
    let email = email_raw.trim().to_lowercase();
    if email.len() > 254 || password.len() > security::MAX_PASSWORD_BYTES {
        return Err(ApiError::InvalidCredentials);
    }
    if !state.limiter.check(
        &format!("login-ip:{}", ratelimit::ip_key(ip)),
        &ratelimit::LOGIN_PER_IP,
    ) || !state
        .limiter
        .check(&format!("login-email:{email}"), &ratelimit::LOGIN_PER_EMAIL)
    {
        return Err(ApiError::RateLimited);
    }
    let now = Utc::now();

    // 1. Look up (short transaction, no lock held during hashing).
    let mut tx = db::auth_tx(&state.pool).await?;
    let row: Option<LoginRow> = sqlx::query_as(
        "SELECT id, password_hash, status, locked_until FROM users WHERE email = $1",
    )
    .bind(&email)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;

    // 2. Verify off the async runtime. Unknown, inactive and locked accounts take the same
    //    path and time and get the same answer, so none of them is distinguishable.
    let usable = row
        .as_ref()
        .filter(|(_, _, status, locked)| status == "active" && !locked.is_some_and(|t| t > now));
    let phc = usable.and_then(|(_, h, _, _)| h.clone());
    let ok = security::verify_password_blocking(&state.hash_permits, password, phc).await;

    let Some((user_id, ..)) = usable.cloned() else {
        return Err(ApiError::InvalidCredentials);
    };
    let mut tx = db::auth_tx(&state.pool).await?;
    if !ok {
        sqlx::query(
            "UPDATE users SET
                 failed_logins = CASE WHEN failed_logins + 1 >= $2 THEN 0 ELSE failed_logins + 1 END,
                 locked_until  = CASE WHEN failed_logins + 1 >= $2 THEN $3 ELSE locked_until END
             WHERE id = $1",
        )
        .bind(user_id)
        .bind(MAX_FAILED_LOGINS)
        .bind(now + LOCKOUT)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        tracing::warn!(%user_id, "login failed");
        return Err(ApiError::InvalidCredentials);
    }

    sqlx::query("UPDATE users SET failed_logins = 0, locked_until = NULL WHERE id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(user_id)
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    tag = "auth",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Signed in", body = LoginResponse),
        (status = 401, description = "Invalid credentials (also for unknown, inactive or locked accounts)", body = ErrorBody),
        (status = 429, description = "Rate limited", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
    )
)]
pub async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(req): ApiJson<LoginRequest>,
) -> ApiResult<Json<LoginResponse>> {
    let user_id = check_password(&state, &ip, &req.email, req.password).await?;
    Ok(Json(start_session(&state, &ip, user_id).await?))
}

/// A new app session for a user whose identity is already established (password, or the
/// demo server's trial accounts).
pub async fn start_session(state: &AppState, ip: &str, user_id: Uuid) -> ApiResult<LoginResponse> {
    let now = Utc::now();
    let mut tx = db::auth_tx(&state.pool).await?;
    let memberships = load_memberships(&mut tx, user_id).await?;
    // The platform super-admin signs in to the apps only from the admin networks (the same
    // ones the admin console is limited to), so a leaked or shared password used from the
    // internet cannot act on every organisation. Same answer as a wrong password.
    if memberships
        .iter()
        .any(|m| m.role == Role::PlatformSuperAdmin)
        && !super::admin::network_allowed(state, ip)
    {
        tracing::warn!(%user_id, "super-admin app login refused outside the admin networks");
        return Err(ApiError::InvalidCredentials);
    }
    let (token, token_hash) = security::new_token();
    let expires_at = now + SESSION_TTL;
    sqlx::query(
        "INSERT INTO sessions (id, token_hash, user_id, expires_at) VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(&token_hash)
    .bind(user_id)
    .bind(expires_at)
    .execute(&mut *tx)
    .await?;
    let me = me_view(&mut tx, user_id, &memberships).await?;
    tx.commit().await?;
    tracing::info!(%user_id, "login");
    Ok(LoginResponse {
        token,
        expires_at,
        me,
    })
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "auth",
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Session revoked"),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn logout(State(state): State<AppState>, p: Principal) -> ApiResult<StatusCode> {
    let mut tx = db::user_tx(&state.pool, p.user_id).await?;
    sqlx::query("UPDATE sessions SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
        .bind(p.session_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/v1/me",
    tag = "auth",
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Current user, active memberships and pending invitations", body = MeResponse),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn me(State(state): State<AppState>, p: Principal) -> ApiResult<Json<MeResponse>> {
    let mut tx = db::user_tx(&state.pool, p.user_id).await?;
    let me = me_view(&mut tx, p.user_id, &p.memberships).await?;
    tx.commit().await?;
    Ok(Json(me))
}

/// Accept or decline an invitation to an organisation (existing accounts only).
#[utoipa::path(
    post,
    path = "/api/v1/invitations/{org_id}/{answer}",
    tag = "auth",
    params(("org_id" = Uuid, Path), ("answer" = String, Path, description = "accept | decline")),
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Answered"),
        (status = 404, description = "No pending invitation", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn respond_invitation(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, answer)): ApiPath<(Uuid, String)>,
) -> ApiResult<StatusCode> {
    let (status, action) = match answer.as_str() {
        "accept" => ("active", "member.joined"),
        "decline" => ("declined", "member.declined"),
        _ => return Err(ApiError::NotFound),
    };
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let role: Option<String> = sqlx::query_scalar(
        "UPDATE memberships SET status = $3, invited_name = NULL
         WHERE org_id = $1 AND user_id = $2 AND status = 'invited' RETURNING role",
    )
    .bind(org_id)
    .bind(p.user_id)
    .bind(status)
    .fetch_optional(&mut *tx)
    .await?;
    let role = role.ok_or(ApiError::NotFound)?;
    audit::record(
        &mut tx,
        Event {
            org_id,
            actor: Some(p.user_id),
            action,
            entity_type: "user",
            entity_id: Some(p.user_id.to_string()),
            data: json!({ "role": role }),
            request_id: p.request_id.as_deref(),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ResetRequest {
    pub email: String,
}

/// Always 202 so the response does not reveal whether the email exists.
#[utoipa::path(
    post,
    path = "/api/v1/auth/password-reset",
    tag = "auth",
    request_body = ResetRequest,
    responses(
        (status = 202, description = "Accepted whether or not the address exists"),
        (status = 429, description = "Rate limited", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
    )
)]
pub async fn request_password_reset(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(req): ApiJson<ResetRequest>,
) -> ApiResult<StatusCode> {
    let email = req.email.trim().to_lowercase();
    if !state.limiter.check(
        &format!("reset-ip:{}", ratelimit::ip_key(&ip)),
        &ratelimit::RESET_PER_IP,
    ) {
        return Err(ApiError::RateLimited);
    }
    // Per-email limit is silent (still 202) so it does not confirm the address exists.
    if email.len() > 254
        || !state
            .limiter
            .check(&format!("reset-email:{email}"), &ratelimit::RESET_PER_EMAIL)
    {
        return Ok(StatusCode::ACCEPTED);
    }
    let mut tx = db::auth_tx(&state.pool).await?;
    let row: Option<(Uuid, String, Option<String>, bool)> = sqlx::query_as(
        "SELECT u.id, u.email::text, u.locale,
                EXISTS (SELECT 1 FROM user_tokens t WHERE t.user_id = u.id AND t.purpose = 'password_reset'
                        AND t.used_at IS NULL AND t.expires_at > now() AND t.created_at > $2)
         FROM users u WHERE u.email = $1 AND u.status = 'active'",
    )
    .bind(&email)
    .bind(Utc::now() - RESET_COOLDOWN)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((user_id, to, locale, recent)) = row else {
        return Ok(StatusCode::ACCEPTED);
    };
    if recent {
        return Ok(StatusCode::ACCEPTED);
    }
    let token = issue_token(&mut tx, user_id, "password_reset", RESET_TOKEN_TTL).await?;
    tx.commit().await?;
    state
        .mailer
        .send(Mail {
            to,
            template: "password_reset",
            locale: locale.unwrap_or_else(|| "ja".into()),
            vars: vec![("token", token)],
        })
        .map_err(ApiError::internal)?;
    Ok(StatusCode::ACCEPTED)
}

pub async fn issue_token(
    tx: &mut db::Tx,
    user_id: Uuid,
    purpose: &str,
    ttl: Duration,
) -> ApiResult<String> {
    let (token, hash) = security::new_token();
    // Only the newest token of a purpose stays usable.
    sqlx::query("UPDATE user_tokens SET used_at = now() WHERE user_id = $1 AND purpose = $2 AND used_at IS NULL")
        .bind(user_id)
        .bind(purpose)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO user_tokens (token_hash, user_id, purpose, expires_at) VALUES ($1, $2, $3, $4)")
        .bind(&hash)
        .bind(user_id)
        .bind(purpose)
        .bind(Utc::now() + ttl)
        .execute(&mut **tx)
        .await?;
    Ok(token)
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct SetPasswordRequest {
    pub token: String,
    pub password: String,
}

/// Redeems an activation or reset token and sets the password. Revokes all sessions.
#[utoipa::path(
    post,
    path = "/api/v1/auth/set-password",
    tag = "auth",
    request_body = SetPasswordRequest,
    responses(
        (status = 204, description = "Password set; all sessions revoked"),
        (status = 422, description = "Weak password or invalid/expired token", body = ErrorBody),
        (status = 429, description = "Rate limited", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
    )
)]
pub async fn set_password(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(req): ApiJson<SetPasswordRequest>,
) -> ApiResult<StatusCode> {
    if !state.limiter.check(
        &format!("setpw-ip:{}", ratelimit::ip_key(&ip)),
        &ratelimit::SET_PASSWORD_PER_IP,
    ) {
        return Err(ApiError::RateLimited);
    }
    security::validate_password(&req.password).map_err(ApiError::Validation)?;
    let hash = security::hash_password_blocking(&state.hash_permits, req.password)
        .await
        .map_err(ApiError::internal)?;
    let mut tx = db::auth_tx(&state.pool).await?;
    let row: Option<(Uuid, String)> = sqlx::query_as(
        "UPDATE user_tokens SET used_at = now()
         WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now()
         RETURNING user_id, purpose",
    )
    .bind(security::token_hash(&req.token))
    .fetch_optional(&mut *tx)
    .await?;
    let (user_id, purpose) = row.ok_or(ApiError::Validation("invalid_or_expired_token"))?;
    let updated = sqlx::query(
        "UPDATE users SET password_hash = $2, failed_logins = 0, locked_until = NULL,
                          status = CASE WHEN status = 'pending' THEN 'active' ELSE status END
         WHERE id = $1 AND status <> 'deactivated'",
    )
    .bind(user_id)
    .bind(&hash)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(ApiError::Validation("invalid_or_expired_token"));
    }
    sqlx::query("UPDATE sessions SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    let memberships = load_memberships(&mut tx, user_id).await?;
    let action = if purpose == "activation" {
        "user.activated"
    } else {
        "user.password_reset"
    };
    for m in &memberships {
        audit::record(
            &mut tx,
            Event {
                org_id: m.org_id,
                actor: Some(user_id),
                action,
                entity_type: "user",
                entity_id: Some(user_id.to_string()),
                data: json!({}),
                request_id: None,
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
