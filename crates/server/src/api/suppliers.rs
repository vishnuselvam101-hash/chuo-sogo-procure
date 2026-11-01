//! Chuo-Sogo's platform-level supplier directory (internal staff) and each customer's
//! decision on the suppliers proposed to it.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use procure_domain::rbac::{self, Access, Permission};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::api::common::{self, Audit};
use crate::db;
use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ApiResult, ErrorBody};
use crate::principal::Principal;

pub const SUPPLIER_STATUSES: [&str; 5] = [
    "discovered",
    "contacted",
    "under_review",
    "verified",
    "inactive",
];

/// Directory maintenance: ManageSuppliers in the platform org.
fn require_directory(p: &Principal) -> ApiResult<Uuid> {
    p.require_platform(Permission::ManageSuppliers)
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct SupplierInput {
    pub name: String,
    /// ISO 3166-1 alpha-2, e.g. `JP`.
    pub country: String,
    pub email: Option<String>,
    pub website: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub notes: String,
}

struct CleanSupplier {
    name: String,
    country: String,
    email: Option<String>,
    website: Option<String>,
    categories: Vec<String>,
    notes: String,
}

fn clean_supplier(s: &SupplierInput) -> ApiResult<CleanSupplier> {
    let country = s.country.trim().to_ascii_uppercase();
    if country.len() != 2 || !country.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(ApiError::Validation("country"));
    }
    let email = match s.email.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
        Some(e) => {
            let ok = e.len() <= 254
                && e.split_once('@')
                    .is_some_and(|(l, d)| !l.is_empty() && d.contains('.') && !d.starts_with('.'))
                && !e.chars().any(|c| c.is_whitespace() || c.is_control());
            if !ok {
                return Err(ApiError::Validation("email"));
            }
            Some(e.to_string())
        }
        None => None,
    };
    let website = match s
        .website
        .as_deref()
        .map(str::trim)
        .filter(|w| !w.is_empty())
    {
        Some(w) => {
            let ok = w.len() <= 300
                && (w.starts_with("https://") || w.starts_with("http://"))
                && !w.chars().any(|c| c.is_whitespace() || c.is_control());
            if !ok {
                return Err(ApiError::Validation("website"));
            }
            Some(w.to_string())
        }
        None => None,
    };
    if s.categories.len() > 20 {
        return Err(ApiError::Validation("categories"));
    }
    let categories = s
        .categories
        .iter()
        .map(|c| common::required(c, 60, false, "categories"))
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(CleanSupplier {
        name: common::required(&s.name, 200, false, "name")?,
        country,
        email,
        website,
        categories,
        notes: common::text(&s.notes, 4000, true, "notes")?,
    })
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct SupplierView {
    pub id: Uuid,
    pub name: String,
    pub country: String,
    pub email: Option<String>,
    pub website: Option<String>,
    pub categories: Vec<String>,
    pub status: String,
    /// Internal notes (staff only).
    pub notes: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct IdResponse {
    pub id: Uuid,
}

#[utoipa::path(
    post,
    path = "/api/v1/suppliers",
    tag = "suppliers",
    request_body = SupplierInput,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Supplier added to the directory (status discovered)", body = IdResponse),
        (status = 403, description = "Not internal procurement staff", body = ErrorBody),
        (status = 422, description = "Invalid input", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn create_supplier(
    State(state): State<AppState>,
    p: Principal,
    ApiJson(body): ApiJson<SupplierInput>,
) -> ApiResult<(StatusCode, Json<IdResponse>)> {
    let platform_org = require_directory(&p)?;
    let s = clean_supplier(&body)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, platform_org).await?;
    db::enable_staff_scope(&mut tx).await?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO suppliers (id, name, country, email, website, categories, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(&s.name)
    .bind(&s.country)
    .bind(&s.email)
    .bind(&s.website)
    .bind(&s.categories)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO supplier_notes (supplier_id, notes) VALUES ($1, $2)")
        .bind(id)
        .bind(&s.notes)
        .execute(&mut *tx)
        .await?;
    Audit {
        org_id: platform_org,
        p: &p,
    }
    .record(
        &mut tx,
        "supplier.created",
        "supplier",
        id,
        json!({ "name": s.name, "country": s.country }),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(IdResponse { id })))
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct SupplierQuery {
    pub status: Option<String>,
    /// Case-insensitive name search.
    pub q: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/suppliers",
    tag = "suppliers",
    params(SupplierQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Directory entries", body = [SupplierView]),
        (status = 403, description = "Not internal staff", body = ErrorBody),
        (status = 422, description = "Unknown status filter", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list_suppliers(
    State(state): State<AppState>,
    p: Principal,
    ApiQuery(q): ApiQuery<SupplierQuery>,
) -> ApiResult<Json<Vec<SupplierView>>> {
    // Any internal staff member may read the directory.
    let platform_org = p
        .memberships
        .iter()
        .find(|m| m.role.is_internal())
        .map(|m| m.org_id)
        .ok_or(ApiError::Forbidden)?;
    if let Some(s) = &q.status
        && !SUPPLIER_STATUSES.contains(&s.as_str())
    {
        return Err(ApiError::Validation("status"));
    }
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
    let mut tx = db::tenant_tx(&state.pool, p.user_id, platform_org).await?;
    db::enable_staff_scope(&mut tx).await?;
    let rows = sqlx::query_as::<_, SupplierView>(
        "SELECT s.id, s.name, s.country, s.email::text AS email, s.website, s.categories, s.status,
                coalesce(n.notes, '') AS notes, s.created_at, s.updated_at
         FROM suppliers s LEFT JOIN supplier_notes n ON n.supplier_id = s.id
         WHERE ($1::text IS NULL OR s.status = $1) AND ($2::text IS NULL OR s.name ILIKE $2)
         ORDER BY s.name, s.id LIMIT 500",
    )
    .bind(q.status)
    .bind(search)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct SupplierStatusBody {
    pub status: String,
    #[serde(default)]
    pub note: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/suppliers/{supplier_id}/status",
    tag = "suppliers",
    params(("supplier_id" = Uuid, Path)),
    request_body = SupplierStatusBody,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Status changed"),
        (status = 403, description = "Not internal procurement staff", body = ErrorBody),
        (status = 404, description = "Unknown supplier", body = ErrorBody),
        (status = 422, description = "Unknown status", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn set_supplier_status(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(supplier_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<SupplierStatusBody>,
) -> ApiResult<StatusCode> {
    let platform_org = require_directory(&p)?;
    if !SUPPLIER_STATUSES.contains(&body.status.as_str()) {
        return Err(ApiError::Validation("status"));
    }
    let note = common::text(&body.note, 2000, true, "note")?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, platform_org).await?;
    db::enable_staff_scope(&mut tx).await?;
    let previous: String =
        sqlx::query_scalar("SELECT status FROM suppliers WHERE id = $1 FOR UPDATE")
            .bind(supplier_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    sqlx::query("UPDATE suppliers SET status = $2, updated_at = now() WHERE id = $1")
        .bind(supplier_id)
        .bind(&body.status)
        .execute(&mut *tx)
        .await?;
    Audit {
        org_id: platform_org,
        p: &p,
    }
    .record(
        &mut tx,
        "supplier.status_changed",
        "supplier",
        supplier_id,
        json!({ "from": previous, "to": body.status, "note": note }),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct OrgSupplierView {
    pub supplier_id: Uuid,
    pub name: String,
    pub country: String,
    pub website: Option<String>,
    pub categories: Vec<String>,
    /// Directory status (Chuo-Sogo's own vetting).
    pub supplier_status: String,
    /// This customer's decision: `proposed`, `approved` or `rejected`.
    pub decision: String,
    pub reason: String,
    pub decided_at: Option<DateTime<Utc>>,
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/suppliers",
    tag = "suppliers",
    params(("org_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Suppliers proposed to this organisation and its decisions", body = [OrgSupplierView]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list_org_suppliers(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
) -> ApiResult<Json<Vec<OrgSupplierView>>> {
    let access = p.access(&state, org_id).await?;
    if access == Access::None {
        return Err(ApiError::NotFound);
    }
    let allowed = [
        Permission::ApproveSuppliers,
        Permission::ReviewQuotations,
        Permission::ManageRfqs,
    ]
    .into_iter()
    .any(|perm| rbac::allows(access, perm));
    if !allowed {
        return Err(ApiError::Forbidden);
    }
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let rows = sqlx::query_as::<_, OrgSupplierView>(
        "SELECT s.id AS supplier_id, s.name, s.country, s.website, s.categories,
                s.status AS supplier_status, a.status AS decision, a.reason, a.decided_at
         FROM customer_supplier_approvals a JOIN suppliers s ON s.id = a.supplier_id
         WHERE a.org_id = $1 ORDER BY s.name, s.id",
    )
    .bind(org_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/suppliers/{supplier_id}/proposal",
    tag = "suppliers",
    params(("org_id" = Uuid, Path), ("supplier_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Supplier proposed to the customer for approval"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation or supplier", body = ErrorBody),
        (status = 409, description = "Already proposed or decided", body = ErrorBody),
        (status = 422, description = "Supplier is not verified", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn propose_supplier(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, supplier_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    let access = p.require(&state, org_id, Permission::ManageRfqs).await?;
    if !matches!(access, Access::Assigned(_)) {
        return Err(ApiError::Forbidden);
    }
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    db::enable_staff_scope(&mut tx).await?;
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM suppliers WHERE id = $1")
        .bind(supplier_id)
        .fetch_optional(&mut *tx)
        .await?;
    match status.as_deref() {
        None => return Err(ApiError::NotFound),
        Some("verified") => {}
        Some(_) => return Err(ApiError::Validation("supplier_not_verified")),
    }
    sqlx::query(
        "INSERT INTO customer_supplier_approvals (org_id, supplier_id, proposed_by)
         VALUES ($1, $2, $3)",
    )
    .bind(org_id)
    .bind(supplier_id)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "supplier.proposed",
            "supplier",
            supplier_id,
            json!({}),
        )
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct SupplierDecisionBody {
    /// `approved` or `rejected`.
    pub decision: String,
    #[serde(default)]
    pub reason: String,
}

#[utoipa::path(
    put,
    path = "/api/v1/orgs/{org_id}/suppliers/{supplier_id}/decision",
    tag = "suppliers",
    params(("org_id" = Uuid, Path), ("supplier_id" = Uuid, Path)),
    request_body = SupplierDecisionBody,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Decision recorded"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Not proposed to this organisation", body = ErrorBody),
        (status = 422, description = "Invalid decision or missing reason", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn decide_supplier(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, supplier_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<SupplierDecisionBody>,
) -> ApiResult<StatusCode> {
    p.require(&state, org_id, Permission::ApproveSuppliers)
        .await?;
    let reason = common::text(&body.reason, 2000, true, "reason")?;
    match body.decision.as_str() {
        "approved" => {}
        "rejected" if !reason.is_empty() => {}
        "rejected" => return Err(ApiError::Validation("reason")),
        _ => return Err(ApiError::Validation("decision")),
    }
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let previous: String = sqlx::query_scalar(
        "SELECT status FROM customer_supplier_approvals
         WHERE org_id = $1 AND supplier_id = $2 FOR UPDATE",
    )
    .bind(org_id)
    .bind(supplier_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    sqlx::query(
        "UPDATE customer_supplier_approvals SET status = $3, reason = $4, decided_by = $5,
                decided_at = now()
         WHERE org_id = $1 AND supplier_id = $2",
    )
    .bind(org_id)
    .bind(supplier_id)
    .bind(&body.decision)
    .bind(&reason)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "supplier.customer_decision",
            "supplier",
            supplier_id,
            json!({ "from": previous, "to": body.decision, "reason": reason }),
        )
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
