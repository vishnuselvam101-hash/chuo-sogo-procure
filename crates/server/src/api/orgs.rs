//! Organisation members, internal service assignments, audit log, and
//! platform-level organisation management.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use procure_domain::rbac::{self, Access, OrgKind, Permission, Role};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::api::auth::{ACTIVATION_TOKEN_TTL, issue_token};
use crate::audit::{self, Event};
use crate::db;
use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ApiResult, ErrorBody};
use crate::mailer::Mail;
use crate::principal::Principal;

fn valid_locale(s: &str) -> bool {
    matches!(s, "ja" | "en" | "ta" | "hi" | "zh-Hans" | "ko")
}

fn normalise_email(raw: &str) -> Result<String, ApiError> {
    let e = raw.trim();
    let ok = e.len() <= 254
        && e.split_once('@')
            .is_some_and(|(l, d)| !l.is_empty() && d.contains('.') && !d.starts_with('.'))
        && !e.chars().any(|c| c.is_whitespace() || c.is_control());
    if ok {
        Ok(e.to_string())
    } else {
        Err(ApiError::Validation("invalid_email"))
    }
}

fn clean_name(raw: &str, max: usize) -> Result<String, ApiError> {
    let n = raw.trim();
    if n.is_empty() || n.chars().count() > max || n.chars().any(char::is_control) {
        return Err(ApiError::Validation("invalid_name"));
    }
    Ok(n.to_string())
}

/// Member management needs ManageInternalUsers in the platform org, ManageOrgUsers elsewhere.
async fn require_member_admin(p: &Principal, state: &AppState, org_id: Uuid) -> ApiResult<()> {
    let access = p.access(state, org_id).await?;
    let perm = match access {
        Access::None => return Err(ApiError::NotFound),
        Access::Member(r) if r.is_internal() => Permission::ManageInternalUsers,
        _ => Permission::ManageOrgUsers,
    };
    if rbac::allows(access, perm) {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

async fn org_kind(tx: &mut db::Tx, org_id: Uuid) -> ApiResult<OrgKind> {
    let kind: Option<String> = sqlx::query_scalar("SELECT kind FROM organisations WHERE id = $1")
        .bind(org_id)
        .fetch_optional(&mut **tx)
        .await?;
    kind.as_deref()
        .and_then(OrgKind::from_code)
        .ok_or(ApiError::NotFound)
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct MemberView {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub membership_status: String,
    pub user_status: String,
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/members",
    tag = "organisations",
    params(("org_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Members and pending invitations", body = [MemberView]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list_members(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
) -> ApiResult<Json<Vec<MemberView>>> {
    require_member_admin(&p, &state, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let rows = member_rows(&mut tx, org_id).await?;
    tx.commit().await?;
    Ok(Json(rows))
}

/// Members and pending invitations of `org_id` (transaction already in that org's context).
pub async fn member_rows(tx: &mut db::Tx, org_id: Uuid) -> ApiResult<Vec<MemberView>> {
    Ok(sqlx::query_as::<_, MemberView>(
        // Invited/declined rows show only what the inviting admin typed; the account's own
        // profile becomes visible after the user accepts.
        "SELECT m.user_id,
                coalesce(u.email::text, m.invited_email::text) AS email,
                coalesce(u.display_name, m.invited_name) AS display_name,
                m.role, m.status AS membership_status,
                CASE WHEN m.status IN ('invited', 'declined') THEN m.status ELSE u.status END AS user_status
         FROM memberships m LEFT JOIN users u ON u.id = m.user_id
         WHERE m.org_id = $1 ORDER BY 3",
    )
    .bind(org_id)
    .fetch_all(&mut **tx)
    .await?)
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct AddMemberRequest {
    pub email: String,
    pub display_name: String,
    pub role: String,
}

/// Identical for new and existing accounts, so it never reveals whether an email is registered.
#[derive(Serialize, utoipa::ToSchema)]
pub struct AddMemberResponse {
    pub status: &'static str,
}

/// Mail to send once the transaction has committed.
pub struct PendingMail(Option<Mail>);

impl PendingMail {
    pub fn send(self, state: &AppState) -> ApiResult<()> {
        match self.0 {
            Some(mail) => state.mailer.send(mail).map_err(ApiError::internal),
            None => Ok(()),
        }
    }
}

/// Finds a user by email or creates a pending one of `kind`; returns (id, created).
/// An existing account of the other kind is rejected (staff and customer accounts never mix).
async fn find_or_create_user(
    tx: &mut db::Tx,
    email: &str,
    name: &str,
    kind: OrgKind,
) -> ApiResult<(Uuid, bool)> {
    let account_kind = if kind == OrgKind::Platform {
        "internal"
    } else {
        "customer"
    };
    db::auth_lookup(tx, true).await?;
    let existing: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, account_kind FROM users WHERE email = $1")
            .bind(email)
            .fetch_optional(&mut **tx)
            .await?;
    let result = match existing {
        Some((_, k)) if k != account_kind => Err(ApiError::Validation("account_kind_mismatch")),
        Some((id, _)) => Ok((id, false)),
        None => {
            let id = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO users (id, email, display_name, account_kind) VALUES ($1, $2, $3, $4)",
            )
            .bind(id)
            .bind(email)
            .bind(name)
            .bind(account_kind)
            .execute(&mut **tx)
            .await?;
            Ok((id, true))
        }
    };
    db::auth_lookup(tx, false).await?;
    result
}

/// Every addition is an invitation the user must accept, for new and existing accounts alike.
/// New accounts additionally get an activation mail (returned, to be sent after commit).
async fn invite_member(
    tx: &mut db::Tx,
    actor: &Principal,
    org_id: Uuid,
    email: &str,
    name: &str,
    role: Role,
) -> ApiResult<PendingMail> {
    let (user_id, created) = find_or_create_user(tx, email, name, role.org_kind()).await?;
    db::set_org(tx, org_id).await?;
    sqlx::query(
        "INSERT INTO memberships (user_id, org_id, role, status, invited_name, invited_email)
         VALUES ($1, $2, $3, 'invited', $4, $5)",
    )
    .bind(user_id)
    .bind(org_id)
    .bind(role.code())
    .bind(name)
    .bind(email)
    .execute(&mut **tx)
    .await?;
    audit::record(
        tx,
        Event {
            org_id,
            actor: Some(actor.user_id),
            action: "member.invited",
            entity_type: "user",
            entity_id: Some(user_id.to_string()),
            data: json!({ "role": role.code() }),
            request_id: actor.request_id.as_deref(),
        },
    )
    .await?;
    if !created {
        return Ok(PendingMail(None));
    }
    let locale: String =
        sqlx::query_scalar("SELECT default_locale FROM organisations WHERE id = $1")
            .bind(org_id)
            .fetch_one(&mut **tx)
            .await?;
    db::auth_lookup(tx, true).await?;
    let token = issue_token(tx, user_id, "activation", ACTIVATION_TOKEN_TTL).await?;
    db::auth_lookup(tx, false).await?;
    Ok(PendingMail(Some(Mail {
        to: email.to_string(),
        template: "activation",
        locale,
        vars: vec![("token", token)],
    })))
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/members",
    tag = "organisations",
    params(("org_id" = Uuid, Path)), request_body = AddMemberRequest,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Invitation created (same response for new and existing accounts)", body = AddMemberResponse),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 409, description = "Already a member or invited", body = ErrorBody),
        (status = 422, description = "Invalid input, role/account kind mismatch", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn add_member(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<AddMemberRequest>,
) -> ApiResult<(StatusCode, Json<AddMemberResponse>)> {
    require_member_admin(&p, &state, org_id).await?;
    let email = normalise_email(&req.email)?;
    let name = clean_name(&req.display_name, 120)?;
    let role = Role::from_code(&req.role).ok_or(ApiError::Validation("invalid_role"))?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    if role.org_kind() != org_kind(&mut tx, org_id).await? {
        return Err(ApiError::Validation("role_not_allowed_in_org"));
    }
    // Only a super admin may create another super admin.
    if role == Role::PlatformSuperAdmin && p.platform_role() != Some(Role::PlatformSuperAdmin) {
        return Err(ApiError::Forbidden);
    }
    let mail = invite_member(&mut tx, &p, org_id, &email, &name, role).await?;
    tx.commit().await?;
    mail.send(&state)?;
    Ok((
        StatusCode::CREATED,
        Json(AddMemberResponse { status: "invited" }),
    ))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct UpdateMemberRequest {
    pub role: Option<String>,
    pub status: Option<String>,
}

#[utoipa::path(
    patch,
    path = "/api/v1/orgs/{org_id}/members/{user_id}",
    tag = "organisations",
    params(("org_id" = Uuid, Path), ("user_id" = Uuid, Path)), request_body = UpdateMemberRequest,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Updated"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Not found", body = ErrorBody),
        (status = 422, description = "Self-modification, last admin, pending invitation, invalid role", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn update_member(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, user_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(req): ApiJson<UpdateMemberRequest>,
) -> ApiResult<StatusCode> {
    require_member_admin(&p, &state, org_id).await?;
    if user_id == p.user_id {
        return Err(ApiError::Validation("cannot_modify_self"));
    }
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    // Serialise membership changes per org so concurrent demotions cannot remove every admin.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 11))")
        .bind(org_id)
        .execute(&mut *tx)
        .await?;
    let kind = org_kind(&mut tx, org_id).await?;
    let current: Option<(String, String)> = sqlx::query_as(
        "SELECT role, status FROM memberships WHERE org_id = $1 AND user_id = $2 FOR UPDATE",
    )
    .bind(org_id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let (old_role, old_status) = current.ok_or(ApiError::NotFound)?;
    // Only the invited user can turn an invitation into an active membership.
    if !matches!(old_status.as_str(), "active" | "deactivated") {
        return Err(ApiError::Validation("invitation_pending"));
    }
    let new_role = match &req.role {
        Some(r) => Role::from_code(r).ok_or(ApiError::Validation("invalid_role"))?,
        None => Role::from_code(&old_role).ok_or(ApiError::Validation("invalid_role"))?,
    };
    if new_role.org_kind() != kind {
        return Err(ApiError::Validation("role_not_allowed_in_org"));
    }
    let touches_super_admin =
        new_role == Role::PlatformSuperAdmin || old_role == Role::PlatformSuperAdmin.code();
    if touches_super_admin && p.platform_role() != Some(Role::PlatformSuperAdmin) {
        return Err(ApiError::Forbidden);
    }
    let new_status = match req.status.as_deref() {
        None => old_status.clone(),
        Some(s @ ("active" | "deactivated")) => s.to_string(),
        Some(_) => return Err(ApiError::Validation("invalid_status")),
    };
    sqlx::query("UPDATE memberships SET role = $3, status = $4 WHERE org_id = $1 AND user_id = $2")
        .bind(org_id)
        .bind(user_id)
        .bind(new_role.code())
        .bind(&new_status)
        .execute(&mut *tx)
        .await?;
    // The org must keep at least one active administrator.
    let admin_role = if kind == OrgKind::Platform {
        Role::PlatformSuperAdmin
    } else {
        Role::CustomerAdmin
    };
    let admins: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM memberships WHERE org_id = $1 AND role = $2 AND status = 'active'",
    )
    .bind(org_id)
    .bind(admin_role.code())
    .fetch_one(&mut *tx)
    .await?;
    if admins == 0 {
        return Err(ApiError::Validation("last_admin"));
    }
    audit::record(
        &mut tx,
        Event {
            org_id,
            actor: Some(p.user_id),
            action: "member.updated",
            entity_type: "user",
            entity_id: Some(user_id.to_string()),
            data: json!({ "role": { "from": old_role, "to": new_role.code() },
                          "status": { "from": old_status, "to": new_status } }),
            request_id: p.request_id.as_deref(),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct AssignmentRequest {
    pub user_id: Uuid,
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/assignments",
    tag = "organisations",
    params(("org_id" = Uuid, Path)), request_body = AssignmentRequest,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Assignment granted"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 409, description = "Already assigned", body = ErrorBody),
        (status = 422, description = "Not internal staff / not a customer org", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn assign_operator(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<AssignmentRequest>,
) -> ApiResult<StatusCode> {
    let platform_org = p.require_platform(Permission::ManageServiceAssignments)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, platform_org).await?;
    let staff_role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM memberships WHERE org_id = $1 AND user_id = $2 AND status = 'active'",
    )
    .bind(platform_org)
    .bind(req.user_id)
    .fetch_optional(&mut *tx)
    .await?;
    if staff_role.is_none() {
        return Err(ApiError::Validation("not_internal_staff"));
    }
    db::set_org(&mut tx, org_id).await?;
    if org_kind(&mut tx, org_id).await? != OrgKind::Customer {
        return Err(ApiError::Validation("not_a_customer_org"));
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO service_assignments (id, org_id, user_id, granted_by) VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(org_id)
    .bind(req.user_id)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    for target in [org_id, platform_org] {
        audit::record(
            &mut tx,
            Event {
                org_id: target,
                actor: Some(p.user_id),
                action: "assignment.granted",
                entity_type: "service_assignment",
                entity_id: Some(id.to_string()),
                data: json!({ "customer_org": org_id, "staff_user": req.user_id }),
                request_id: p.request_id.as_deref(),
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::CREATED)
}

#[utoipa::path(
    delete,
    path = "/api/v1/orgs/{org_id}/assignments/{user_id}",
    tag = "organisations",
    params(("org_id" = Uuid, Path), ("user_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Assignment revoked"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "No active assignment", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn revoke_operator(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, user_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    let platform_org = p.require_platform(Permission::ManageServiceAssignments)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let revoked: Option<Uuid> = sqlx::query_scalar(
        "UPDATE service_assignments SET revoked_at = now()
         WHERE org_id = $1 AND user_id = $2 AND revoked_at IS NULL RETURNING id",
    )
    .bind(org_id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let id = revoked.ok_or(ApiError::NotFound)?;
    for target in [org_id, platform_org] {
        audit::record(
            &mut tx,
            Event {
                org_id: target,
                actor: Some(p.user_id),
                action: "assignment.revoked",
                entity_type: "service_assignment",
                entity_id: Some(id.to_string()),
                data: json!({ "customer_org": org_id, "staff_user": user_id }),
                request_id: p.request_id.as_deref(),
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AuditQuery {
    pub limit: Option<i64>,
    pub before_seq: Option<i64>,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct AuditView {
    pub seq: i64,
    pub actor_user_id: Option<Uuid>,
    pub action: String,
    pub entity_type: String,
    pub entity_id: Option<String>,
    #[schema(value_type = Object)]
    pub data: serde_json::Value,
    pub request_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct AuditResponse {
    pub events: Vec<AuditView>,
    pub chain_valid: bool,
    pub first_broken_seq: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/audit",
    tag = "organisations",
    params(("org_id" = Uuid, Path), AuditQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Newest events first, plus chain verification", body = AuditResponse),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list_audit(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiQuery(q): ApiQuery<AuditQuery>,
) -> ApiResult<Json<AuditResponse>> {
    p.require(&state, org_id, Permission::ViewAudit).await?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let resp = audit_page(&mut tx, org_id, q.before_seq, limit).await?;
    tx.commit().await?;
    Ok(Json(resp))
}

/// One page of an organisation's audit log, newest first, with chain verification
/// (transaction already in that org's context).
pub async fn audit_page(
    tx: &mut db::Tx,
    org_id: Uuid,
    before_seq: Option<i64>,
    limit: i64,
) -> ApiResult<AuditResponse> {
    let events = sqlx::query_as::<_, AuditView>(
        "SELECT seq, actor_user_id, action, entity_type, entity_id, data, request_id, created_at
         FROM audit_events WHERE org_id = $1 AND ($2::bigint IS NULL OR seq < $2)
         ORDER BY seq DESC LIMIT $3",
    )
    .bind(org_id)
    .bind(before_seq)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;
    let broken = audit::verify_chain(tx, org_id).await?;
    Ok(AuditResponse {
        events,
        chain_valid: broken.is_none(),
        first_broken_seq: broken,
    })
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreateOrgRequest {
    pub name: String,
    pub default_locale: Option<String>,
    pub timezone: Option<String>,
    pub admin_email: String,
    pub admin_name: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct CreateOrgResponse {
    pub org_id: Uuid,
    pub admin: &'static str,
}

#[utoipa::path(
    post,
    path = "/api/v1/platform/organisations",
    tag = "organisations",
    request_body = CreateOrgRequest,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Organisation created; administrator invited", body = CreateOrgResponse),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 422, description = "Invalid input or account kind mismatch", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn create_organisation(
    State(state): State<AppState>,
    p: Principal,
    ApiJson(req): ApiJson<CreateOrgRequest>,
) -> ApiResult<(StatusCode, Json<CreateOrgResponse>)> {
    let platform_org = p.require_platform(Permission::ManageOrganisations)?;
    let name = clean_name(&req.name, 200)?;
    let locale = req.default_locale.unwrap_or_else(|| "ja".into());
    if !valid_locale(&locale) {
        return Err(ApiError::Validation("invalid_locale"));
    }
    let timezone = req.timezone.unwrap_or_else(|| "Asia/Tokyo".into());
    let email = normalise_email(&req.admin_email)?;
    let admin_name = clean_name(&req.admin_name, 120)?;

    let mut tx = db::user_tx(&state.pool, p.user_id).await?;
    let tz_ok: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_timezone_names WHERE name = $1)")
            .bind(&timezone)
            .fetch_one(&mut *tx)
            .await?;
    if !tz_ok {
        return Err(ApiError::Validation("invalid_timezone"));
    }
    db::enable_platform_scope(&mut tx).await?;
    let org_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO organisations (id, kind, name, default_locale, timezone) VALUES ($1, 'customer', $2, $3, $4)",
    )
    .bind(org_id)
    .bind(&name)
    .bind(&locale)
    .bind(&timezone)
    .execute(&mut *tx)
    .await?;
    for target in [platform_org, org_id] {
        audit::record(
            &mut tx,
            Event {
                org_id: target,
                actor: Some(p.user_id),
                action: "organisation.created",
                entity_type: "organisation",
                entity_id: Some(org_id.to_string()),
                data: json!({ "name": name, "default_locale": locale, "timezone": timezone }),
                request_id: p.request_id.as_deref(),
            },
        )
        .await?;
    }
    let mail = invite_member(
        &mut tx,
        &p,
        org_id,
        &email,
        &admin_name,
        Role::CustomerAdmin,
    )
    .await?;
    tx.commit().await?;
    mail.send(&state)?;
    Ok((
        StatusCode::CREATED,
        Json(CreateOrgResponse {
            org_id,
            admin: "invited",
        }),
    ))
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct OrgView {
    pub id: Uuid,
    pub kind: String,
    pub name: String,
    pub default_locale: String,
    pub timezone: String,
    pub status: String,
}

#[utoipa::path(
    get,
    path = "/api/v1/platform/organisations",
    tag = "organisations",
    security(("bearer" = [])),
    responses(
        (status = 200, description = "All organisations", body = [OrgView]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list_organisations(
    State(state): State<AppState>,
    p: Principal,
) -> ApiResult<Json<Vec<OrgView>>> {
    p.require_platform(Permission::ManageOrganisations)?;
    let mut tx = db::user_tx(&state.pool, p.user_id).await?;
    db::enable_platform_scope(&mut tx).await?;
    let rows = sqlx::query_as::<_, OrgView>(
        "SELECT id, kind, name, default_locale, timezone, status FROM organisations ORDER BY kind DESC, name",
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}
