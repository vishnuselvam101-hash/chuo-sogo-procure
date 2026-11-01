//! Admin console API (`/api/admin/v1`): Chuo-Sogo staff only, from allowed networks, with a
//! second factor (TOTP) on every session. Sessions issued here work only here.
//!
//! Wherever the customer API already implements an operation with the right permission checks
//! (organisations, members, assignments), the admin route calls that same handler with the
//! admin principal, so the rules exist once.

use axum::Json;
use axum::extract::{FromRequestParts, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use chrono::{DateTime, Duration, Utc};
use procure_domain::rbac::{self, Permission};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::api::auth::{UserView, check_password, issue_token};
use crate::api::common::{self, Audit};
use crate::api::orgs;
use crate::audit;
use crate::db;
use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ApiResult, ErrorBody};
use crate::mfa;
use crate::principal::{self, ClientIp, Principal};
use crate::ratelimit;
use crate::security;

pub const ADMIN_IDLE_TIMEOUT: Duration = Duration::minutes(30);
pub const ADMIN_SESSION_TTL: Duration = Duration::hours(8);
const CHALLENGE_TTL: Duration = Duration::minutes(5);
const ENROLLMENT_TTL: Duration = Duration::minutes(15);
const ISSUER: &str = "Chuo-Sogo Procure Admin";

/// An authenticated admin-console session: second factor done, request from an allowed
/// network, caller holds a role in the platform (Chuo-Sogo) organisation.
pub struct AdminPrincipal(pub Principal);

pub(crate) fn network_allowed(state: &AppState, ip: &str) -> bool {
    match &state.admin_networks {
        // Only for in-process tests (no peer address); production always has a list.
        None => true,
        Some(nets) => ip
            .parse::<std::net::IpAddr>()
            .is_ok_and(|addr| nets.iter().any(|n| n.contains(&addr))),
    }
}

impl FromRequestParts<AppState> for AdminPrincipal {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Ok(ClientIp(ip)) = ClientIp::from_request_parts(parts, state).await;
        if !network_allowed(state, &ip) {
            // Same answer as a missing route: the admin API does not exist for this network.
            return Err(ApiError::NotFound);
        }
        let token = principal::bearer(parts).ok_or(ApiError::Unauthenticated)?;
        let p = principal::authenticate(state, parts, token, "admin", ADMIN_IDLE_TIMEOUT).await?;
        if p.platform_role().is_none() {
            return Err(ApiError::Forbidden);
        }
        Ok(AdminPrincipal(p))
    }
}

async fn require_network(state: &AppState, ip: &str) -> ApiResult<()> {
    if network_allowed(state, ip) {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

fn secret_box(state: &AppState) -> ApiResult<&mfa::SecretBox> {
    state
        .secret_box
        .as_deref()
        .ok_or(ApiError::Conflict("admin_mfa_unavailable"))
}

// ---------------------------------------------------------------------------------------
// Sign-in: password → (first time: enrol an authenticator) → code → admin session.

#[derive(Deserialize, utoipa::ToSchema)]
pub struct AdminLoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct AdminLoginStep {
    /// `mfa` (enter the authenticator code) or `enroll` (set up an authenticator first).
    pub step: &'static str,
    /// Short-lived token for the next step (5 min for `mfa`, 15 min for `enroll`).
    pub token: String,
}

#[utoipa::path(
    operation_id = "admin_login",
    post,
    path = "/api/admin/v1/auth/login",
    tag = "admin",
    request_body = AdminLoginRequest,
    responses(
        (status = 200, description = "Password accepted; second step required", body = AdminLoginStep),
        (status = 401, description = "Invalid credentials (also for non-staff accounts)", body = ErrorBody),
        (status = 404, description = "Not available from this network", body = ErrorBody),
        (status = 409, description = "Second factor not configured on this server", body = ErrorBody),
        (status = 429, description = "Too many attempts", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
    )
)]
pub async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(req): ApiJson<AdminLoginRequest>,
) -> ApiResult<Json<AdminLoginStep>> {
    require_network(&state, &ip).await?;
    secret_box(&state)?;
    let user_id = check_password(&state, &ip, &req.email, req.password).await?;
    let mut tx = db::auth_tx(&state.pool).await?;
    let memberships = principal::load_memberships(&mut tx, user_id).await?;
    db::auth_lookup(&mut tx, true).await?;
    if !memberships.iter().any(|m| m.role.is_internal()) {
        // Customer accounts get the same answer as a wrong password.
        return Err(ApiError::InvalidCredentials);
    }
    let enrolled: bool =
        sqlx::query_scalar("SELECT totp_enabled_at IS NOT NULL FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?;
    let (step, token) = if enrolled {
        (
            "mfa",
            issue_token(&mut tx, user_id, "mfa_challenge", CHALLENGE_TTL).await?,
        )
    } else {
        (
            "enroll",
            issue_token(&mut tx, user_id, "totp_enrollment", ENROLLMENT_TTL).await?,
        )
    };
    tx.commit().await?;
    Ok(Json(AdminLoginStep { step, token }))
}

/// Looks up an unused, unexpired step token without consuming it.
async fn peek_token(tx: &mut db::Tx, token: &str, purpose: &str) -> ApiResult<Uuid> {
    sqlx::query_scalar(
        "SELECT user_id FROM user_tokens
         WHERE token_hash = $1 AND purpose = $2 AND used_at IS NULL AND expires_at > now()",
    )
    .bind(security::token_hash(token))
    .bind(purpose)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::Unauthenticated)
}

async fn consume_token(tx: &mut db::Tx, token: &str) -> ApiResult<()> {
    sqlx::query("UPDATE user_tokens SET used_at = now() WHERE token_hash = $1")
        .bind(security::token_hash(token))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct AdminSession {
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

async fn start_session(tx: &mut db::Tx, user_id: Uuid) -> ApiResult<AdminSession> {
    let (token, hash) = security::new_token();
    let expires_at = Utc::now() + ADMIN_SESSION_TTL;
    sqlx::query(
        "INSERT INTO sessions (id, token_hash, user_id, expires_at, audience, mfa_at)
         VALUES ($1, $2, $3, $4, 'admin', now())",
    )
    .bind(Uuid::now_v7())
    .bind(&hash)
    .bind(user_id)
    .bind(expires_at)
    .execute(&mut **tx)
    .await?;
    Ok(AdminSession { token, expires_at })
}

/// Checks a TOTP code for `user_id`, recording the step (no replay) or a failed attempt
/// (counts towards the same lockout as passwords).
async fn check_code(state: &AppState, tx: &mut db::Tx, user_id: Uuid, code: &str) -> ApiResult<()> {
    if !state
        .limiter
        .check(&format!("mfa:{user_id}"), &ratelimit::MFA_PER_USER)
    {
        return Err(ApiError::RateLimited);
    }
    let (sealed, last_step): (Option<Vec<u8>>, Option<i64>) = sqlx::query_as(
        "SELECT totp_secret_enc, totp_last_step FROM users WHERE id = $1 FOR UPDATE",
    )
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    let secret = sealed
        .and_then(|s| secret_box(state).ok()?.open(&s))
        .ok_or(ApiError::Unauthenticated)?;
    match mfa::verify(&secret, code, Utc::now().timestamp(), last_step) {
        Some(step) => {
            sqlx::query("UPDATE users SET totp_last_step = $2, failed_logins = 0 WHERE id = $1")
                .bind(user_id)
                .bind(step)
                .execute(&mut **tx)
                .await?;
            Ok(())
        }
        None => Err(ApiError::InvalidCredentials),
    }
}

async fn record_failed_code(state: &AppState, user_id: Uuid) -> ApiResult<()> {
    let mut tx = db::auth_tx(&state.pool).await?;
    sqlx::query(
        "UPDATE users SET
             failed_logins = CASE WHEN failed_logins + 1 >= $2 THEN 0 ELSE failed_logins + 1 END,
             locked_until  = CASE WHEN failed_logins + 1 >= $2 THEN $3 ELSE locked_until END
         WHERE id = $1",
    )
    .bind(user_id)
    .bind(crate::api::auth::MAX_FAILED_LOGINS)
    .bind(Utc::now() + crate::api::auth::LOCKOUT)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn audit_platform(
    tx: &mut db::Tx,
    user_id: Uuid,
    action: &str,
    data: serde_json::Value,
) -> ApiResult<()> {
    let memberships = principal::load_memberships(tx, user_id).await?;
    if let Some(platform) = memberships.iter().find(|m| m.role.is_internal()) {
        audit::record(
            tx,
            audit::Event {
                org_id: platform.org_id,
                actor: Some(user_id),
                action,
                entity_type: "user",
                entity_id: Some(user_id.to_string()),
                data,
                request_id: None,
            },
        )
        .await?;
    }
    Ok(())
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct CodeRequest {
    pub token: String,
    /// 6-digit code from the authenticator app.
    pub code: String,
}

#[utoipa::path(
    operation_id = "admin_verify_mfa",
    post,
    path = "/api/admin/v1/auth/mfa",
    tag = "admin",
    request_body = CodeRequest,
    responses(
        (status = 200, description = "Admin session started", body = AdminSession),
        (status = 401, description = "Wrong or expired code/token", body = ErrorBody),
        (status = 404, description = "Not available from this network", body = ErrorBody),
        (status = 429, description = "Too many attempts", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
    )
)]
pub async fn verify_mfa(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(req): ApiJson<CodeRequest>,
) -> ApiResult<Json<AdminSession>> {
    require_network(&state, &ip).await?;
    let mut tx = db::auth_tx(&state.pool).await?;
    let user_id = peek_token(&mut tx, &req.token, "mfa_challenge").await?;
    if let Err(e) = check_code(&state, &mut tx, user_id, &req.code).await {
        drop(tx);
        if matches!(e, ApiError::InvalidCredentials) {
            record_failed_code(&state, user_id).await?;
        }
        return Err(e);
    }
    consume_token(&mut tx, &req.token).await?;
    let session = start_session(&mut tx, user_id).await?;
    audit_platform(&mut tx, user_id, "admin.login", json!({ "ip": ip })).await?;
    tx.commit().await?;
    Ok(Json(session))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct EnrollRequest {
    pub token: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct Enrollment {
    /// Base32 secret to type into an authenticator app.
    pub secret: String,
    /// `otpauth://` URI (for a QR code).
    pub otpauth_uri: String,
}

#[utoipa::path(
    operation_id = "admin_enroll",
    post,
    path = "/api/admin/v1/auth/totp/enroll",
    tag = "admin",
    request_body = EnrollRequest,
    responses(
        (status = 200, description = "New authenticator secret (not active until confirmed)", body = Enrollment),
        (status = 401, description = "Invalid or expired token", body = ErrorBody),
        (status = 404, description = "Not available from this network", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
    )
)]
pub async fn enroll(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(req): ApiJson<EnrollRequest>,
) -> ApiResult<Json<Enrollment>> {
    require_network(&state, &ip).await?;
    let sb = secret_box(&state)?;
    let mut tx = db::auth_tx(&state.pool).await?;
    let user_id = peek_token(&mut tx, &req.token, "totp_enrollment").await?;
    let (email, enabled): (String, bool) = sqlx::query_as(
        "SELECT email::text, totp_enabled_at IS NOT NULL FROM users WHERE id = $1 FOR UPDATE",
    )
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;
    if enabled {
        return Err(ApiError::Conflict("already_enrolled"));
    }
    let secret = mfa::new_secret();
    let sealed = sb
        .seal(&secret)
        .ok_or_else(|| ApiError::internal(std::io::Error::other("seal failed")))?;
    sqlx::query("UPDATE users SET totp_secret_enc = $2, totp_last_step = NULL WHERE id = $1")
        .bind(user_id)
        .bind(&sealed)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(Enrollment {
        secret: mfa::base32(&secret),
        otpauth_uri: mfa::otpauth_uri(ISSUER, &email, &secret),
    }))
}

#[utoipa::path(
    operation_id = "admin_confirm_enrollment",
    post,
    path = "/api/admin/v1/auth/totp/confirm",
    tag = "admin",
    request_body = CodeRequest,
    responses(
        (status = 200, description = "Authenticator confirmed; admin session started", body = AdminSession),
        (status = 401, description = "Wrong code or expired token", body = ErrorBody),
        (status = 404, description = "Not available from this network", body = ErrorBody),
        (status = 429, description = "Too many attempts", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
    )
)]
pub async fn confirm_enrollment(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(req): ApiJson<CodeRequest>,
) -> ApiResult<Json<AdminSession>> {
    require_network(&state, &ip).await?;
    let mut tx = db::auth_tx(&state.pool).await?;
    let user_id = peek_token(&mut tx, &req.token, "totp_enrollment").await?;
    if let Err(e) = check_code(&state, &mut tx, user_id, &req.code).await {
        drop(tx);
        if matches!(e, ApiError::InvalidCredentials) {
            record_failed_code(&state, user_id).await?;
        }
        return Err(e);
    }
    sqlx::query("UPDATE users SET totp_enabled_at = now() WHERE id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    consume_token(&mut tx, &req.token).await?;
    let session = start_session(&mut tx, user_id).await?;
    audit_platform(&mut tx, user_id, "admin.mfa_enrolled", json!({ "ip": ip })).await?;
    tx.commit().await?;
    Ok(Json(session))
}

#[utoipa::path(
    operation_id = "admin_logout",
    post,
    path = "/api/admin/v1/auth/logout",
    tag = "admin",
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Session ended"),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn logout(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
) -> ApiResult<StatusCode> {
    let mut tx = db::user_tx(&state.pool, p.user_id).await?;
    sqlx::query("UPDATE sessions SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
        .bind(p.session_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------------------
// Identity.

#[derive(Serialize, utoipa::ToSchema)]
pub struct AdminMe {
    pub user: UserView,
    pub platform_org_id: Uuid,
    pub role: &'static str,
    pub permissions: Vec<&'static str>,
}

#[utoipa::path(
    operation_id = "admin_me",
    get,
    path = "/api/admin/v1/me",
    tag = "admin",
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Signed-in administrator", body = AdminMe),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn me(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
) -> ApiResult<Json<AdminMe>> {
    let platform = p
        .memberships
        .iter()
        .find(|m| m.role.is_internal())
        .ok_or(ApiError::Forbidden)?;
    let mut tx = db::user_tx(&state.pool, p.user_id).await?;
    let user = sqlx::query_as::<_, UserView>(
        "SELECT id, email::text AS email, display_name, locale, account_kind FROM users WHERE id = $1",
    )
    .bind(p.user_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(AdminMe {
        user,
        platform_org_id: platform.org_id,
        role: platform.role.code(),
        permissions: rbac::home_permissions(platform.role)
            .iter()
            .map(|p| p.code())
            .collect(),
    }))
}

// ---------------------------------------------------------------------------------------
// Organisations.

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct OrgOverview {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub status: String,
    pub default_locale: String,
    pub timezone: String,
    pub created_at: DateTime<Utc>,
    #[sqlx(skip)]
    pub members: i64,
    #[sqlx(skip)]
    pub open_requests: i64,
    #[sqlx(skip)]
    pub assigned_staff: i64,
}

#[utoipa::path(
    operation_id = "admin_list_orgs",
    get,
    path = "/api/admin/v1/orgs",
    tag = "admin",
    security(("bearer" = [])),
    responses(
        (status = 200, description = "All organisations with activity counts", body = [OrgOverview]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn list_orgs(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
) -> ApiResult<Json<Vec<OrgOverview>>> {
    let platform = p.require_platform(Permission::ManageOrganisations)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, platform).await?;
    db::enable_platform_scope(&mut tx).await?;
    let mut orgs = sqlx::query_as::<_, OrgOverview>(
        "SELECT id, name, kind, status, default_locale, timezone, created_at FROM organisations
         ORDER BY kind DESC, name",
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    // Counts per organisation inside that organisation's own context (RLS).
    for o in &mut orgs {
        let mut tx = db::tenant_tx(&state.pool, p.user_id, o.id).await?;
        let (members, open, staff): (i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM memberships WHERE org_id = $1 AND status = 'active'),
                    (SELECT count(*) FROM procurement_requests
                     WHERE org_id = $1 AND status NOT IN ('draft', 'closed', 'cancelled', 'rejected')),
                    (SELECT count(*) FROM service_assignments WHERE org_id = $1 AND revoked_at IS NULL)",
        )
        .bind(o.id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        (o.members, o.open_requests, o.assigned_staff) = (members, open, staff);
    }
    Ok(Json(orgs))
}

#[utoipa::path(
    operation_id = "admin_create_org",
    post,
    path = "/api/admin/v1/orgs",
    tag = "admin",
    request_body = orgs::CreateOrgRequest,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Organisation created; administrator invited", body = orgs::CreateOrgResponse),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 422, description = "Invalid input", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn create_org(
    state: State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    body: ApiJson<orgs::CreateOrgRequest>,
) -> ApiResult<(StatusCode, Json<orgs::CreateOrgResponse>)> {
    orgs::create_organisation(state, p, body).await
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct OrgUpdate {
    pub name: Option<String>,
    /// `active` or `suspended` (suspended: nobody can work in it; data is kept).
    pub status: Option<String>,
    pub default_locale: Option<String>,
    pub timezone: Option<String>,
}

#[utoipa::path(
    operation_id = "admin_update_org",
    patch,
    path = "/api/admin/v1/orgs/{org_id}",
    tag = "admin",
    params(("org_id" = Uuid, Path)),
    request_body = OrgUpdate,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Updated"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation", body = ErrorBody),
        (status = 409, description = "The platform organisation cannot be suspended", body = ErrorBody),
        (status = 422, description = "Invalid input", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn update_org(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<OrgUpdate>,
) -> ApiResult<StatusCode> {
    let platform = p.require_platform(Permission::ManageOrganisations)?;
    let name = body
        .name
        .as_deref()
        .map(|n| common::required(n, 200, false, "name"))
        .transpose()?;
    if let Some(s) = &body.status
        && !["active", "suspended"].contains(&s.as_str())
    {
        return Err(ApiError::Validation("status"));
    }
    if let Some(l) = &body.default_locale
        && !["ja", "en", "ta", "hi", "zh-Hans", "ko"].contains(&l.as_str())
    {
        return Err(ApiError::Validation("default_locale"));
    }
    let timezone = body
        .timezone
        .as_deref()
        .map(|t| common::required(t, 64, false, "timezone"))
        .transpose()?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, platform).await?;
    db::enable_platform_scope(&mut tx).await?;
    if let Some(tz) = &timezone {
        let known: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_timezone_names WHERE name = $1)")
                .bind(tz)
                .fetch_one(&mut *tx)
                .await?;
        if !known {
            return Err(ApiError::Validation("timezone"));
        }
    }
    let kind: String =
        sqlx::query_scalar("SELECT kind FROM organisations WHERE id = $1 FOR UPDATE")
            .bind(org_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    if kind == "platform" && body.status.as_deref() == Some("suspended") {
        return Err(ApiError::Conflict("cannot_suspend_platform"));
    }
    sqlx::query(
        "UPDATE organisations SET name = coalesce($2, name), status = coalesce($3, status),
                default_locale = coalesce($4, default_locale), timezone = coalesce($5, timezone)
         WHERE id = $1",
    )
    .bind(org_id)
    .bind(&name)
    .bind(&body.status)
    .bind(&body.default_locale)
    .bind(&timezone)
    .execute(&mut *tx)
    .await?;
    let data = json!({ "org": org_id, "name": name, "status": body.status,
                       "default_locale": body.default_locale, "timezone": timezone });
    for target in [platform, org_id] {
        Audit {
            org_id: target,
            p: &p,
        }
        .record(&mut tx, "org.updated", "organisation", org_id, data.clone())
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    operation_id = "admin_org_members",
    get,
    path = "/api/admin/v1/orgs/{org_id}/members",
    tag = "admin",
    params(("org_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Members of any organisation", body = [orgs::MemberView]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn org_members(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath(org_id): ApiPath<Uuid>,
) -> ApiResult<Json<Vec<orgs::MemberView>>> {
    p.require_platform(Permission::ManageOrganisations)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let rows = orgs::member_rows(&mut tx, org_id).await?;
    tx.commit().await?;
    Ok(Json(rows))
}

// ---------------------------------------------------------------------------------------
// Rules per organisation.

#[derive(Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct OrgSettings {
    /// Hours Chuo-Sogo has to identify products after a request is submitted.
    pub identify_sla_hours: i32,
    /// Hours to collect quotations when an RFQ has no response date.
    pub chase_default_hours: i32,
    /// Whether AI suggestions may be used for this organisation.
    pub ai_enabled: bool,
    /// Default invoice tax rate in basis points (1000 = 10%).
    pub default_tax_rate_bps: i32,
}

impl Default for OrgSettings {
    fn default() -> Self {
        OrgSettings {
            identify_sla_hours: 24,
            chase_default_hours: 72,
            ai_enabled: true,
            default_tax_rate_bps: 1000,
        }
    }
}

/// The organisation's rules, or the defaults. Reads in the org's own context.
pub async fn load_settings(tx: &mut db::Tx, org_id: Uuid) -> ApiResult<OrgSettings> {
    Ok(sqlx::query_as::<_, OrgSettings>(
        "SELECT identify_sla_hours, chase_default_hours, ai_enabled, default_tax_rate_bps
         FROM org_settings WHERE org_id = $1",
    )
    .bind(org_id)
    .fetch_optional(&mut **tx)
    .await?
    .unwrap_or_default())
}

#[utoipa::path(
    operation_id = "admin_get_settings",
    get,
    path = "/api/admin/v1/orgs/{org_id}/settings",
    tag = "admin",
    params(("org_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Rules for the organisation (defaults if never set)", body = OrgSettings),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn get_settings(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath(org_id): ApiPath<Uuid>,
) -> ApiResult<Json<OrgSettings>> {
    p.require_platform(Permission::ManageOrganisations)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let s = load_settings(&mut tx, org_id).await?;
    tx.commit().await?;
    Ok(Json(s))
}

#[utoipa::path(
    operation_id = "admin_put_settings",
    put,
    path = "/api/admin/v1/orgs/{org_id}/settings",
    tag = "admin",
    params(("org_id" = Uuid, Path)),
    request_body = OrgSettings,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Saved"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation", body = ErrorBody),
        (status = 422, description = "Out of range", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn put_settings(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<OrgSettings>,
) -> ApiResult<StatusCode> {
    let platform = p.require_platform(Permission::ManageOrganisations)?;
    if !(1..=720).contains(&body.identify_sla_hours) {
        return Err(ApiError::Validation("identify_sla_hours"));
    }
    if !(1..=2160).contains(&body.chase_default_hours) {
        return Err(ApiError::Validation("chase_default_hours"));
    }
    if !(0..=10_000).contains(&body.default_tax_rate_bps) {
        return Err(ApiError::Validation("default_tax_rate_bps"));
    }
    let mut tx = db::tenant_tx(&state.pool, p.user_id, platform).await?;
    db::enable_platform_scope(&mut tx).await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM organisations WHERE id = $1)")
            .bind(org_id)
            .fetch_one(&mut *tx)
            .await?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    db::set_org(&mut tx, org_id).await?;
    sqlx::query(
        "INSERT INTO org_settings (org_id, identify_sla_hours, chase_default_hours, ai_enabled,
                                   default_tax_rate_bps, updated_by)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (org_id) DO UPDATE SET identify_sla_hours = EXCLUDED.identify_sla_hours,
             chase_default_hours = EXCLUDED.chase_default_hours, ai_enabled = EXCLUDED.ai_enabled,
             default_tax_rate_bps = EXCLUDED.default_tax_rate_bps, updated_by = EXCLUDED.updated_by,
             updated_at = now()",
    )
    .bind(org_id)
    .bind(body.identify_sla_hours)
    .bind(body.chase_default_hours)
    .bind(body.ai_enabled)
    .bind(body.default_tax_rate_bps)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    let data = serde_json::to_value(&body).map_err(ApiError::internal)?;
    for target in [platform, org_id] {
        Audit {
            org_id: target,
            p: &p,
        }
        .record(
            &mut tx,
            "org.settings_updated",
            "organisation",
            org_id,
            data.clone(),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------------------
// Accounts.

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct AccountView {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    /// `internal` or `customer`.
    pub account_kind: String,
    pub status: String,
    pub mfa_enabled: bool,
    pub locked: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct AccountQuery {
    /// Part of the email address or name.
    pub q: Option<String>,
}

#[utoipa::path(
    operation_id = "admin_list_accounts",
    get,
    path = "/api/admin/v1/accounts",
    tag = "admin",
    params(AccountQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Accounts matching the search (at most 200)", body = [AccountView]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn list_accounts(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiQuery(q): ApiQuery<AccountQuery>,
) -> ApiResult<Json<Vec<AccountView>>> {
    let platform = p.require_platform(Permission::ManageOrganisations)?;
    let search =
        q.q.as_deref()
            .map(|s| common::text(s, 100, false, "q"))
            .transpose()?
            .filter(|s| !s.is_empty())
            .map(|s| {
                format!(
                    "%{}%",
                    s.replace('\\', "\\\\")
                        .replace('%', "\\%")
                        .replace('_', "\\_")
                )
            });
    let mut tx = db::tenant_tx(&state.pool, p.user_id, platform).await?;
    db::enable_platform_scope(&mut tx).await?;
    let rows = sqlx::query_as::<_, AccountView>(
        "SELECT id, email::text AS email, display_name, account_kind, status,
                totp_enabled_at IS NOT NULL AS mfa_enabled,
                coalesce(locked_until > now(), false) AS locked, created_at
         FROM users WHERE ($1::text IS NULL OR email ILIKE $1 OR display_name ILIKE $1)
         ORDER BY email LIMIT 200",
    )
    .bind(search)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct AccountUpdate {
    /// `active` or `deactivated` (deactivating ends all sessions).
    pub status: Option<String>,
    /// Remove the authenticator so the person enrols again at next admin sign-in.
    #[serde(default)]
    pub reset_mfa: bool,
    /// Lift a lockout after too many failed sign-ins.
    #[serde(default)]
    pub unlock: bool,
}

#[utoipa::path(
    operation_id = "admin_update_account",
    patch,
    path = "/api/admin/v1/accounts/{user_id}",
    tag = "admin",
    params(("user_id" = Uuid, Path)),
    request_body = AccountUpdate,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Updated"),
        (status = 403, description = "Not permitted (or own account)", body = ErrorBody),
        (status = 404, description = "Unknown account", body = ErrorBody),
        (status = 409, description = "Not possible for this account (e.g. never activated)", body = ErrorBody),
        (status = 422, description = "Invalid input", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn update_account(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath(user_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<AccountUpdate>,
) -> ApiResult<StatusCode> {
    let platform = p.require_platform(Permission::ManageInternalUsers)?;
    if user_id == p.user_id {
        return Err(ApiError::Forbidden);
    }
    if let Some(s) = &body.status
        && !["active", "deactivated"].contains(&s.as_str())
    {
        return Err(ApiError::Validation("status"));
    }
    let mut tx = db::tenant_tx(&state.pool, p.user_id, platform).await?;
    db::enable_platform_scope(&mut tx).await?;
    let (current, has_password): (String, bool) = sqlx::query_as(
        "SELECT status, password_hash IS NOT NULL FROM users WHERE id = $1 FOR UPDATE",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    if body.status.as_deref() == Some("active") && !has_password {
        return Err(ApiError::Conflict("not_activated"));
    }
    db::auth_lookup(&mut tx, true).await?;
    sqlx::query(
        "UPDATE users SET status = coalesce($2, status),
                totp_secret_enc = CASE WHEN $3 THEN NULL ELSE totp_secret_enc END,
                totp_enabled_at = CASE WHEN $3 THEN NULL ELSE totp_enabled_at END,
                totp_last_step = CASE WHEN $3 THEN NULL ELSE totp_last_step END,
                failed_logins = CASE WHEN $4 THEN 0 ELSE failed_logins END,
                locked_until = CASE WHEN $4 THEN NULL ELSE locked_until END
         WHERE id = $1",
    )
    .bind(user_id)
    .bind(&body.status)
    .bind(body.reset_mfa)
    .bind(body.unlock)
    .execute(&mut *tx)
    .await?;
    if body.status.as_deref() == Some("deactivated") || body.reset_mfa {
        // Deactivation ends every session; an MFA reset ends admin sessions.
        sqlx::query(
            "UPDATE sessions SET revoked_at = now()
             WHERE user_id = $1 AND revoked_at IS NULL AND ($2 OR audience = 'admin')",
        )
        .bind(user_id)
        .bind(body.status.as_deref() == Some("deactivated"))
        .execute(&mut *tx)
        .await?;
    }
    db::auth_lookup(&mut tx, false).await?;
    Audit {
        org_id: platform,
        p: &p,
    }
    .record(
        &mut tx,
        "account.updated",
        "user",
        user_id,
        json!({ "from": current, "status": body.status, "reset_mfa": body.reset_mfa,
                    "unlock": body.unlock }),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------------------
// Staff assignments (reuse the customer-API handlers and their checks).

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct AssignmentView {
    pub user_id: Uuid,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[utoipa::path(
    operation_id = "admin_list_assignments",
    get,
    path = "/api/admin/v1/orgs/{org_id}/assignments",
    tag = "admin",
    params(("org_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Staff currently assigned to the organisation", body = [AssignmentView]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn list_assignments(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath(org_id): ApiPath<Uuid>,
) -> ApiResult<Json<Vec<AssignmentView>>> {
    p.require_platform(Permission::ManageServiceAssignments)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let rows = sqlx::query_as::<_, AssignmentView>(
        "SELECT a.user_id, u.display_name, u.email::text AS email, a.created_at
         FROM service_assignments a LEFT JOIN users u ON u.id = a.user_id
         WHERE a.org_id = $1 AND a.revoked_at IS NULL ORDER BY u.display_name",
    )
    .bind(org_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[utoipa::path(
    operation_id = "admin_assign",
    post,
    path = "/api/admin/v1/orgs/{org_id}/assignments",
    tag = "admin",
    params(("org_id" = Uuid, Path)),
    request_body = orgs::AssignmentRequest,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Staff member assigned"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 409, description = "Already assigned", body = ErrorBody),
        (status = 422, description = "Not internal staff, or not a customer organisation", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn assign(
    state: State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    path: ApiPath<Uuid>,
    body: ApiJson<orgs::AssignmentRequest>,
) -> ApiResult<StatusCode> {
    orgs::assign_operator(state, p, path, body).await
}

#[utoipa::path(
    operation_id = "admin_unassign",
    delete,
    path = "/api/admin/v1/orgs/{org_id}/assignments/{user_id}",
    tag = "admin",
    params(("org_id" = Uuid, Path), ("user_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Assignment revoked"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "No active assignment", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn unassign(
    state: State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    path: ApiPath<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    orgs::revoke_operator(state, p, path).await
}

// ---------------------------------------------------------------------------------------
// Audit log of any organisation.

#[derive(Deserialize, utoipa::IntoParams)]
pub struct AdminAuditQuery {
    pub limit: Option<i64>,
    pub before_seq: Option<i64>,
}

#[utoipa::path(
    operation_id = "admin_org_audit",
    get,
    path = "/api/admin/v1/orgs/{org_id}/audit",
    tag = "admin",
    params(("org_id" = Uuid, Path), AdminAuditQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Audit events with hash-chain verification", body = orgs::AuditResponse),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn org_audit(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiQuery(q): ApiQuery<AdminAuditQuery>,
) -> ApiResult<Json<orgs::AuditResponse>> {
    p.require_platform(Permission::ViewAudit)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let resp = orgs::audit_page(
        &mut tx,
        org_id,
        q.before_seq,
        q.limit.unwrap_or(100).clamp(1, 500),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(resp))
}

// ---------------------------------------------------------------------------------------
// Monitoring.

#[derive(Serialize, utoipa::ToSchema)]
pub struct OrgHealth {
    pub org_id: Uuid,
    pub name: String,
    pub open_requests: i64,
    pub overdue_tasks: i64,
    pub disputed_invoices: i64,
    pub failed_ai_24h: i64,
    /// Hash chain of the organisation's audit log intact.
    pub audit_chain_valid: bool,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct SystemHealth {
    pub server_version: &'static str,
    pub schema_version: Option<i64>,
    pub database_bytes: i64,
    pub ai_provider: String,
    pub ai_model: String,
    pub active_sessions: i64,
    pub accounts_active: i64,
    pub accounts_locked: i64,
    pub orgs: Vec<OrgHealth>,
}

#[utoipa::path(
    operation_id = "admin_health",
    get,
    path = "/api/admin/v1/health",
    tag = "admin",
    security(("bearer" = [])),
    responses(
        (status = 200, description = "System and per-organisation health", body = SystemHealth),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn health(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
) -> ApiResult<Json<SystemHealth>> {
    let platform = p.require_platform(Permission::ViewSystemHealth)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, platform).await?;
    db::enable_platform_scope(&mut tx).await?;
    let schema_version: Option<i64> =
        sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations WHERE success")
            .fetch_one(&mut *tx)
            .await?;
    let database_bytes: i64 = sqlx::query_scalar("SELECT pg_database_size(current_database())")
        .fetch_one(&mut *tx)
        .await?;
    let (accounts_active, accounts_locked): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status = 'active'),
                count(*) FILTER (WHERE locked_until > now()) FROM users",
    )
    .fetch_one(&mut *tx)
    .await?;
    db::auth_lookup(&mut tx, true).await?;
    let active_sessions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sessions WHERE revoked_at IS NULL AND expires_at > now()",
    )
    .fetch_one(&mut *tx)
    .await?;
    db::auth_lookup(&mut tx, false).await?;
    let orgs: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, name FROM organisations WHERE kind = 'customer' AND status = 'active' ORDER BY name",
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let mut out = Vec::with_capacity(orgs.len());
    for (org_id, name) in orgs {
        let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
        db::enable_staff_scope(&mut tx).await?;
        let (open_requests, overdue_tasks, disputed_invoices, failed_ai_24h): (i64, i64, i64, i64) =
            sqlx::query_as(
                "SELECT (SELECT count(*) FROM procurement_requests
                         WHERE status NOT IN ('draft', 'closed', 'cancelled', 'rejected')),
                        (SELECT count(*) FROM tasks WHERE status IN ('open', 'in_progress')
                         AND due_at < now()),
                        (SELECT count(*) FROM invoices WHERE match_status = 'disputed'),
                        (SELECT count(*) FROM ai_suggestions WHERE status = 'failed'
                         AND created_at > now() - interval '24 hours')",
            )
            .fetch_one(&mut *tx)
            .await?;
        let audit_chain_valid = audit::verify_chain(&mut tx, org_id).await?.is_none();
        tx.commit().await?;
        out.push(OrgHealth {
            org_id,
            name,
            open_requests,
            overdue_tasks,
            disputed_invoices,
            failed_ai_24h,
            audit_chain_valid,
        });
    }
    Ok(Json(SystemHealth {
        server_version: env!("CARGO_PKG_VERSION"),
        schema_version,
        database_bytes,
        ai_provider: state.ai.name().to_string(),
        ai_model: state.ai.model().to_string(),
        active_sessions,
        accounts_active,
        accounts_locked,
        orgs: out,
    }))
}
