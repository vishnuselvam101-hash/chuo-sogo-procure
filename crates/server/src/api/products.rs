//! Chuo-Sogo's product identity records: manufacturer part numbers with specifications and
//! the public evidence an operator relied on. Platform-level, maintained by staff; a customer
//! can read only the records its own request items are linked to.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use procure_domain::rbac::Permission;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::types::Json as DbJson;
use uuid::Uuid;

use crate::AppState;
use crate::api::common::{self, Audit};
use crate::api::suppliers::IdResponse;
use crate::db;
use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ApiResult, ErrorBody};
use crate::principal::Principal;

pub const EVIDENCE_KINDS: [&str; 5] = [
    "datasheet",
    "catalogue",
    "manufacturer_page",
    "supplier_confirmation",
    "other",
];
const MAX_SPECS: usize = 50;

fn staff_org(p: &Principal) -> ApiResult<Uuid> {
    p.memberships
        .iter()
        .find(|m| m.role.is_internal())
        .map(|m| m.org_id)
        .ok_or(ApiError::Forbidden)
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct EvidenceInput {
    /// `datasheet`, `catalogue`, `manufacturer_page`, `supplier_confirmation` or `other`.
    pub kind: String,
    /// Public https source. Either a URL or a note is required.
    pub url: Option<String>,
    #[serde(default)]
    pub note: String,
}

struct CleanEvidence {
    kind: String,
    url: Option<String>,
    note: String,
}

fn clean_evidence(e: &EvidenceInput) -> ApiResult<CleanEvidence> {
    if !EVIDENCE_KINDS.contains(&e.kind.as_str()) {
        return Err(ApiError::Validation("kind"));
    }
    let url = match e.url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
        Some(u) => {
            let ok = u.len() <= 500
                && u.starts_with("https://")
                && u.len() > "https://".len()
                && !u.chars().any(|c| c.is_whitespace() || c.is_control());
            if !ok {
                return Err(ApiError::Validation("url"));
            }
            Some(u.to_string())
        }
        None => None,
    };
    let note = common::text(&e.note, 2000, true, "note")?;
    if url.is_none() && note.is_empty() {
        return Err(ApiError::Validation("evidence"));
    }
    Ok(CleanEvidence {
        kind: e.kind.clone(),
        url,
        note,
    })
}

async fn insert_evidence(
    tx: &mut db::Tx,
    product_id: Uuid,
    e: &CleanEvidence,
    user: Uuid,
) -> ApiResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO product_evidence (id, product_id, kind, url, note, added_by)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(product_id)
    .bind(&e.kind)
    .bind(&e.url)
    .bind(&e.note)
    .bind(user)
    .execute(&mut **tx)
    .await?;
    Ok(id)
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ProductInput {
    pub manufacturer: String,
    pub mpn: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub category: String,
    /// Flat key/value specifications, e.g. `{"bore_mm": "20"}`.
    #[serde(default)]
    pub specs: BTreeMap<String, String>,
    #[serde(default)]
    pub evidence: Vec<EvidenceInput>,
}

fn clean_specs(specs: &BTreeMap<String, String>) -> ApiResult<BTreeMap<String, String>> {
    if specs.len() > MAX_SPECS {
        return Err(ApiError::Validation("specs"));
    }
    specs
        .iter()
        .map(|(k, v)| {
            Ok((
                common::required(k, 60, false, "specs")?,
                common::text(v, 200, false, "specs")?,
            ))
        })
        .collect()
}

#[utoipa::path(
    post,
    path = "/api/v1/products",
    tag = "products",
    request_body = ProductInput,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Draft product record created", body = IdResponse),
        (status = 403, description = "Not procurement staff", body = ErrorBody),
        (status = 409, description = "Manufacturer + part number already recorded", body = ErrorBody),
        (status = 422, description = "Invalid input", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn create_product(
    State(state): State<AppState>,
    p: Principal,
    ApiJson(body): ApiJson<ProductInput>,
) -> ApiResult<(StatusCode, Json<IdResponse>)> {
    let platform_org = p.require_platform(Permission::ManageProducts)?;
    let manufacturer = common::required(&body.manufacturer, 200, false, "manufacturer")?;
    let mpn = common::required(&body.mpn, 100, false, "mpn")?;
    let description = common::text(&body.description, 1000, true, "description")?;
    let category = common::text(&body.category, 100, false, "category")?;
    let specs = clean_specs(&body.specs)?;
    if body.evidence.len() > 20 {
        return Err(ApiError::Validation("evidence"));
    }
    let evidence = body
        .evidence
        .iter()
        .map(clean_evidence)
        .collect::<ApiResult<Vec<_>>>()?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, platform_org).await?;
    db::enable_staff_scope(&mut tx).await?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO product_identities (id, manufacturer, mpn, description, category, specs, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(&manufacturer)
    .bind(&mpn)
    .bind(&description)
    .bind(&category)
    .bind(DbJson(&specs))
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    for e in &evidence {
        insert_evidence(&mut tx, id, e, p.user_id).await?;
    }
    Audit {
        org_id: platform_org,
        p: &p,
    }
    .record(
        &mut tx,
        "product.created",
        "product_identity",
        id,
        json!({ "manufacturer": manufacturer, "mpn": mpn, "evidence": evidence.len() }),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(IdResponse { id })))
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct ProductView {
    pub id: Uuid,
    pub manufacturer: String,
    pub mpn: String,
    pub description: String,
    pub category: String,
    #[schema(value_type = BTreeMap<String, String>)]
    pub specs: DbJson<BTreeMap<String, String>>,
    /// `draft`, `verified` or `retired`.
    pub status: String,
    pub evidence_count: i64,
    pub verified_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

const PRODUCT_COLUMNS: &str = "p.id, p.manufacturer, p.mpn, p.description, p.category, p.specs,
    p.status, (SELECT count(*) FROM product_evidence e WHERE e.product_id = p.id) AS evidence_count,
    p.verified_at, p.created_at";

#[derive(Deserialize, utoipa::IntoParams)]
pub struct ProductQuery {
    /// Case-insensitive search in manufacturer, part number and description.
    pub q: Option<String>,
    pub status: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/products",
    tag = "products",
    params(ProductQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Product records (staff)", body = [ProductView]),
        (status = 403, description = "Not internal staff", body = ErrorBody),
        (status = 422, description = "Unknown status filter", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list_products(
    State(state): State<AppState>,
    p: Principal,
    ApiQuery(q): ApiQuery<ProductQuery>,
) -> ApiResult<Json<Vec<ProductView>>> {
    let platform_org = staff_org(&p)?;
    if let Some(s) = &q.status
        && !["draft", "verified", "retired"].contains(&s.as_str())
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
    let rows = sqlx::query_as::<_, ProductView>(&format!(
        "SELECT {PRODUCT_COLUMNS} FROM product_identities p
         WHERE ($1::text IS NULL OR p.status = $1)
           AND ($2::text IS NULL OR p.mpn ILIKE $2 OR p.manufacturer ILIKE $2 OR p.description ILIKE $2)
         ORDER BY p.manufacturer, p.mpn LIMIT 200"
    ))
    .bind(q.status)
    .bind(search)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct EvidenceView {
    pub id: Uuid,
    pub kind: String,
    pub url: Option<String>,
    pub note: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ProductDetail {
    pub product: ProductView,
    pub evidence: Vec<EvidenceView>,
}

/// `internal`: staff see every piece of evidence; customers only public sources (URL
/// evidence other than supplier confirmations), never notes.
async fn product_detail(tx: &mut db::Tx, id: Uuid, internal: bool) -> ApiResult<ProductDetail> {
    let product = sqlx::query_as::<_, ProductView>(&format!(
        "SELECT {PRODUCT_COLUMNS} FROM product_identities p WHERE p.id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    let evidence = sqlx::query_as::<_, EvidenceView>(
        "SELECT id, kind, url, CASE WHEN $2 THEN note ELSE '' END AS note, created_at
         FROM product_evidence
         WHERE product_id = $1
           AND ($2 OR (url IS NOT NULL AND kind <> 'supplier_confirmation'))
         ORDER BY created_at, id",
    )
    .bind(id)
    .bind(internal)
    .fetch_all(&mut **tx)
    .await?;
    Ok(ProductDetail { product, evidence })
}

#[utoipa::path(
    get,
    path = "/api/v1/products/{product_id}",
    tag = "products",
    params(("product_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Product record with evidence (staff)", body = ProductDetail),
        (status = 403, description = "Not internal staff", body = ErrorBody),
        (status = 404, description = "Unknown product", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn get_product(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<Json<ProductDetail>> {
    let platform_org = staff_org(&p)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, platform_org).await?;
    db::enable_staff_scope(&mut tx).await?;
    let detail = product_detail(&mut tx, id, true).await?;
    tx.commit().await?;
    Ok(Json(detail))
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/products/{product_id}",
    tag = "products",
    params(("org_id" = Uuid, Path), ("product_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Product record behind one of this organisation's request items", body = ProductDetail),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown, or not linked to this organisation", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn get_org_product(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<ProductDetail>> {
    let (_, view) = common::request_view(&p, &state, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    // RLS already limits products to this org's items; requesters see only their own.
    let linked: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM request_items i JOIN procurement_requests r ON r.id = i.request_id
                        WHERE i.product_identity_id = $1 AND ($2::uuid IS NULL OR r.requested_by = $2))",
    )
    .bind(id)
    .bind(view.owner_filter())
    .fetch_one(&mut *tx)
    .await?;
    if !linked {
        return Err(ApiError::NotFound);
    }
    let detail = product_detail(&mut tx, id, false).await?;
    tx.commit().await?;
    Ok(Json(detail))
}

#[utoipa::path(
    post,
    path = "/api/v1/products/{product_id}/evidence",
    tag = "products",
    params(("product_id" = Uuid, Path)),
    request_body = EvidenceInput,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Evidence added (evidence is never edited or removed)", body = IdResponse),
        (status = 403, description = "Not procurement staff", body = ErrorBody),
        (status = 404, description = "Unknown product", body = ErrorBody),
        (status = 422, description = "Invalid evidence", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn add_evidence(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(product_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<EvidenceInput>,
) -> ApiResult<(StatusCode, Json<IdResponse>)> {
    let platform_org = p.require_platform(Permission::ManageProducts)?;
    let e = clean_evidence(&body)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, platform_org).await?;
    db::enable_staff_scope(&mut tx).await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM product_identities WHERE id = $1)")
            .bind(product_id)
            .fetch_one(&mut *tx)
            .await?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    let id = insert_evidence(&mut tx, product_id, &e, p.user_id).await?;
    Audit {
        org_id: platform_org,
        p: &p,
    }
    .record(
        &mut tx,
        "product.evidence_added",
        "product_identity",
        product_id,
        json!({ "kind": e.kind, "url": e.url }),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(IdResponse { id })))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ProductStatusBody {
    /// `verified`, `retired` or `draft`.
    pub status: String,
    #[serde(default)]
    pub note: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/products/{product_id}/status",
    tag = "products",
    params(("product_id" = Uuid, Path)),
    request_body = ProductStatusBody,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Status changed"),
        (status = 403, description = "Not procurement staff", body = ErrorBody),
        (status = 404, description = "Unknown product", body = ErrorBody),
        (status = 409, description = "Verification needs at least one piece of evidence", body = ErrorBody),
        (status = 422, description = "Unknown status", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn set_product_status(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(product_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<ProductStatusBody>,
) -> ApiResult<StatusCode> {
    let platform_org = p.require_platform(Permission::ManageProducts)?;
    if !["verified", "retired", "draft"].contains(&body.status.as_str()) {
        return Err(ApiError::Validation("status"));
    }
    let note = common::text(&body.note, 2000, true, "note")?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, platform_org).await?;
    db::enable_staff_scope(&mut tx).await?;
    let previous: String =
        sqlx::query_scalar("SELECT status FROM product_identities WHERE id = $1 FOR UPDATE")
            .bind(product_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    if body.status == "verified" {
        // At least one checkable public source, not just a note.
        let evidence: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM product_evidence WHERE product_id = $1 AND url IS NOT NULL",
        )
        .bind(product_id)
        .fetch_one(&mut *tx)
        .await?;
        if evidence == 0 {
            return Err(ApiError::Conflict("evidence_required"));
        }
    }
    sqlx::query(
        "UPDATE product_identities SET status = $2,
                verified_by = CASE WHEN $2 = 'verified' THEN $3 ELSE verified_by END,
                verified_at = CASE WHEN $2 = 'verified' THEN now() ELSE verified_at END
         WHERE id = $1",
    )
    .bind(product_id)
    .bind(&body.status)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    Audit {
        org_id: platform_org,
        p: &p,
    }
    .record(
        &mut tx,
        "product.status_changed",
        "product_identity",
        product_id,
        json!({ "from": previous, "to": body.status, "note": note }),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
