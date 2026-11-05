//! Supplier lifecycle (L6): compliance documents in the platform-level supplier directory
//! (our staff, ManageSuppliers), what a customer may see of them, and each customer's
//! supplier scorecards (computed from its own records, agents/supplier_watch.rs).

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::IntoResponse;
use base64::Engine;
use chrono::{DateTime, NaiveDate, Utc};
use procure_domain::compliance::{self, DocKind};
use procure_domain::rbac::{self, Access, Permission};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::agents::supplier_watch::{self as watch, SupplierScorecard};
use crate::api::common::{self, Audit};
use crate::api::suppliers::IdResponse;
use crate::db;
use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ApiResult, ErrorBody};
use crate::principal::Principal;
use crate::storage::{self, MAX_ATTACHMENT_BYTES};

/// Documents per supplier, retired ones included.
const MAX_DOCS_PER_SUPPLIER: i64 = 200;

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ComplianceDocInput {
    /// `iso9001` | `iso14001` | `iatf16949` | `qualified_invoice_registration` |
    /// `anti_social_check` | `insurance` | `nda` | `other`
    pub kind: String,
    /// Certificate or policy number. For `qualified_invoice_registration`, the registration
    /// number (`T` + 13 digits, check digit verified; spaces, hyphens and full-width
    /// characters are accepted and normalised).
    #[serde(default)]
    pub reference: String,
    #[serde(default)]
    pub title: String,
    pub issued_on: Option<NaiveDate>,
    /// Valid through this date (inclusive). Absent: does not expire.
    pub expires_on: Option<NaiveDate>,
    /// Shown to customers that work with this supplier. Default false.
    #[serde(default)]
    pub customer_visible: bool,
    /// Internal notes (staff only; never shown to customers).
    #[serde(default)]
    pub notes: String,
    /// Optional file: name and standard base64 content (same types as attachments).
    pub file_name: Option<String>,
    pub data_base64: Option<String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ComplianceDocUpdate {
    #[serde(default)]
    pub reference: String,
    #[serde(default)]
    pub title: String,
    pub issued_on: Option<NaiveDate>,
    pub expires_on: Option<NaiveDate>,
    pub customer_visible: bool,
    #[serde(default)]
    pub notes: String,
    /// Retire (superseded or withdrawn): hidden from customers and from the agent.
    #[serde(default)]
    pub retired: bool,
}

/// Staff view, with internal notes and who verified the document.
#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct StaffComplianceDoc {
    pub id: Uuid,
    pub supplier_id: Uuid,
    pub kind: String,
    pub reference: String,
    pub title: String,
    pub issued_on: Option<NaiveDate>,
    pub expires_on: Option<NaiveDate>,
    pub customer_visible: bool,
    pub notes: String,
    pub file_name: Option<String>,
    pub content_type: Option<String>,
    pub size_bytes: Option<i64>,
    pub verified_by: Option<Uuid>,
    pub verified_at: Option<DateTime<Utc>>,
    pub retired_at: Option<DateTime<Utc>>,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// What a customer sees: no internal notes, no staff identities, no file.
#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct CustomerComplianceDoc {
    pub id: Uuid,
    pub supplier_id: Uuid,
    pub kind: String,
    pub reference: String,
    pub title: String,
    pub issued_on: Option<NaiveDate>,
    pub expires_on: Option<NaiveDate>,
    /// Checked by Chuo-Sogo staff.
    pub verified: bool,
}

struct CleanDoc {
    reference: String,
    title: String,
    notes: String,
}

fn clean_doc(
    kind: DocKind,
    reference: &str,
    title: &str,
    notes: &str,
    issued_on: Option<NaiveDate>,
    expires_on: Option<NaiveDate>,
) -> ApiResult<CleanDoc> {
    let reference = if kind == DocKind::QualifiedInvoiceRegistration {
        compliance::normalize_invoice_registration_number(reference)
            .ok_or(ApiError::Validation("reference"))?
    } else {
        common::text(reference, 100, false, "reference")?
    };
    if let (Some(i), Some(e)) = (issued_on, expires_on)
        && e < i
    {
        return Err(ApiError::Validation("expires_on"));
    }
    Ok(CleanDoc {
        reference,
        title: common::text(title, 200, false, "title")?,
        notes: common::text(notes, 4000, true, "notes")?,
    })
}

const STAFF_COLUMNS: &str = "d.id, d.supplier_id, d.kind, d.reference, d.title, d.issued_on, \
     d.expires_on, d.customer_visible, coalesce(n.notes, '') AS notes, d.file_name, \
     d.content_type, d.size_bytes, d.verified_by, d.verified_at, d.retired_at, d.created_by, \
     d.created_at, d.updated_at";

/// A directory transaction for our staff (ManageSuppliers in the platform org).
async fn staff_tx(state: &AppState, p: &Principal) -> ApiResult<(Uuid, db::Tx)> {
    let platform_org = p.require_platform(Permission::ManageSuppliers)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, platform_org).await?;
    db::enable_staff_scope(&mut tx).await?;
    Ok((platform_org, tx))
}

#[utoipa::path(
    operation_id = "supplier_watch_create_doc",
    post,
    path = "/api/v1/suppliers/{supplier_id}/compliance-docs",
    tag = "suppliers",
    params(("supplier_id" = Uuid, Path)),
    request_body = ComplianceDocInput,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Document recorded", body = IdResponse),
        (status = 403, description = "Not internal procurement staff", body = ErrorBody),
        (status = 404, description = "Unknown supplier", body = ErrorBody),
        (status = 413, description = "Larger than 10 MB"),
        (status = 422, description = "Invalid kind, registration number, dates or file", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn create_doc(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(supplier_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<ComplianceDocInput>,
) -> ApiResult<(StatusCode, Json<IdResponse>)> {
    p.require_platform(Permission::ManageSuppliers)?;
    let kind = DocKind::from_code(&body.kind).ok_or(ApiError::Validation("kind"))?;
    let doc = clean_doc(
        kind,
        &body.reference,
        &body.title,
        &body.notes,
        body.issued_on,
        body.expires_on,
    )?;
    let file = match (&body.file_name, &body.data_base64) {
        (None, None) => None,
        (Some(name), Some(data)) => {
            let name = storage::clean_file_name(name).ok_or(ApiError::Validation("file_name"))?;
            let _permit = state
                .upload_permits
                .acquire()
                .await
                .map_err(ApiError::internal)?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data.as_bytes())
                .map_err(|_| ApiError::BadRequest("invalid_base64"))?;
            if bytes.is_empty() || bytes.len() > MAX_ATTACHMENT_BYTES {
                return Err(ApiError::Validation("file_size"));
            }
            let content_type = storage::content_type_for(&name, &bytes)
                .ok_or(ApiError::Validation("file_type"))?;
            // Bytes first (content-addressed, idempotent): a failed transaction leaves at
            // most an unreferenced file.
            let hash = state
                .storage
                .put(&bytes)
                .await
                .map_err(ApiError::internal)?;
            let size = i64::try_from(bytes.len()).map_err(|_| ApiError::Validation("file_size"))?;
            Some((name, content_type, size, hash))
        }
        _ => return Err(ApiError::Validation("file")),
    };
    let (platform_org, mut tx) = staff_tx(&state, &p).await?;
    let known: Option<i64> = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM supplier_compliance_docs WHERE supplier_id = s.id)
         FROM suppliers s WHERE s.id = $1 FOR UPDATE",
    )
    .bind(supplier_id)
    .fetch_optional(&mut *tx)
    .await?;
    match known {
        None => return Err(ApiError::NotFound),
        Some(n) if n >= MAX_DOCS_PER_SUPPLIER => {
            return Err(ApiError::Validation("too_many_documents"));
        }
        Some(_) => {}
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO supplier_compliance_docs (id, supplier_id, kind, reference, title,
             issued_on, expires_on, customer_visible, file_name, content_type, size_bytes,
             sha256, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(id)
    .bind(supplier_id)
    .bind(kind.code())
    .bind(&doc.reference)
    .bind(&doc.title)
    .bind(body.issued_on)
    .bind(body.expires_on)
    .bind(body.customer_visible)
    .bind(file.as_ref().map(|f| f.0.clone()))
    .bind(file.as_ref().map(|f| f.1))
    .bind(file.as_ref().map(|f| f.2))
    .bind(file.as_ref().map(|f| f.3.to_vec()))
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO supplier_compliance_doc_notes (doc_id, notes) VALUES ($1, $2)")
        .bind(id)
        .bind(&doc.notes)
        .execute(&mut *tx)
        .await?;
    Audit {
        org_id: platform_org,
        p: &p,
    }
    .record(
        &mut tx,
        "supplier.compliance_doc_added",
        "supplier_compliance_doc",
        id,
        json!({ "supplier_id": supplier_id, "kind": kind.code(), "reference": doc.reference,
                "issued_on": body.issued_on, "expires_on": body.expires_on,
                "customer_visible": body.customer_visible,
                "sha256": file.as_ref().map(|f| hex::encode(f.3)) }),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(IdResponse { id })))
}

#[utoipa::path(
    operation_id = "supplier_watch_list_docs",
    get,
    path = "/api/v1/suppliers/{supplier_id}/compliance-docs",
    tag = "suppliers",
    params(("supplier_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Every document of the supplier, retired ones included", body = [StaffComplianceDoc]),
        (status = 403, description = "Not internal procurement staff", body = ErrorBody),
        (status = 404, description = "Unknown supplier", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list_docs(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(supplier_id): ApiPath<Uuid>,
) -> ApiResult<Json<Vec<StaffComplianceDoc>>> {
    let (_, mut tx) = staff_tx(&state, &p).await?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM suppliers WHERE id = $1)")
        .bind(supplier_id)
        .fetch_one(&mut *tx)
        .await?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    let rows = sqlx::query_as::<_, StaffComplianceDoc>(&format!(
        "SELECT {STAFF_COLUMNS}
         FROM supplier_compliance_docs d
         LEFT JOIN supplier_compliance_doc_notes n ON n.doc_id = d.id
         WHERE d.supplier_id = $1
         ORDER BY d.retired_at NULLS FIRST, d.kind, d.expires_on NULLS LAST, d.id"
    ))
    .bind(supplier_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[utoipa::path(
    operation_id = "supplier_watch_update_doc",
    put,
    path = "/api/v1/suppliers/{supplier_id}/compliance-docs/{doc_id}",
    tag = "suppliers",
    params(("supplier_id" = Uuid, Path), ("doc_id" = Uuid, Path)),
    request_body = ComplianceDocUpdate,
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Updated; a changed reference or date clears the verification", body = StaffComplianceDoc),
        (status = 403, description = "Not internal procurement staff", body = ErrorBody),
        (status = 404, description = "Unknown document", body = ErrorBody),
        (status = 409, description = "Retired documents do not change", body = ErrorBody),
        (status = 422, description = "Invalid registration number or dates", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn update_doc(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((supplier_id, doc_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<ComplianceDocUpdate>,
) -> ApiResult<Json<StaffComplianceDoc>> {
    let (platform_org, mut tx) = staff_tx(&state, &p).await?;
    let before = load_staff_doc(&mut tx, supplier_id, doc_id, true).await?;
    if before.retired_at.is_some() {
        return Err(ApiError::Conflict("document_retired"));
    }
    let kind = DocKind::from_code(&before.kind)
        .ok_or_else(|| ApiError::internal(std::io::Error::other("unknown document kind")))?;
    let doc = clean_doc(
        kind,
        &body.reference,
        &body.title,
        &body.notes,
        body.issued_on,
        body.expires_on,
    )?;
    let facts_changed = doc.reference != before.reference
        || body.issued_on != before.issued_on
        || body.expires_on != before.expires_on;
    sqlx::query(
        "UPDATE supplier_compliance_docs SET reference = $2, title = $3, issued_on = $4,
                expires_on = $5, customer_visible = $6,
                verified_by = CASE WHEN $7 THEN NULL ELSE verified_by END,
                verified_at = CASE WHEN $7 THEN NULL ELSE verified_at END,
                retired_at = CASE WHEN $8 THEN now() ELSE NULL END, updated_at = now()
         WHERE id = $1",
    )
    .bind(doc_id)
    .bind(&doc.reference)
    .bind(&doc.title)
    .bind(body.issued_on)
    .bind(body.expires_on)
    .bind(body.customer_visible)
    .bind(facts_changed)
    .bind(body.retired)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO supplier_compliance_doc_notes (doc_id, notes) VALUES ($1, $2)
         ON CONFLICT (doc_id) DO UPDATE SET notes = EXCLUDED.notes",
    )
    .bind(doc_id)
    .bind(&doc.notes)
    .execute(&mut *tx)
    .await?;
    Audit {
        org_id: platform_org,
        p: &p,
    }
    .record(
        &mut tx,
        "supplier.compliance_doc_updated",
        "supplier_compliance_doc",
        doc_id,
        json!({
            "supplier_id": supplier_id,
            "before": { "reference": before.reference, "issued_on": before.issued_on,
                        "expires_on": before.expires_on,
                        "customer_visible": before.customer_visible },
            "after": { "reference": doc.reference, "issued_on": body.issued_on,
                       "expires_on": body.expires_on,
                       "customer_visible": body.customer_visible },
            "verification_cleared": facts_changed && before.verified_at.is_some(),
            "retired": body.retired,
        }),
    )
    .await?;
    let after = load_staff_doc(&mut tx, supplier_id, doc_id, false).await?;
    tx.commit().await?;
    Ok(Json(after))
}

async fn load_staff_doc(
    tx: &mut db::Tx,
    supplier_id: Uuid,
    doc_id: Uuid,
    lock: bool,
) -> ApiResult<StaffComplianceDoc> {
    let lock = if lock { "FOR UPDATE OF d" } else { "" };
    sqlx::query_as::<_, StaffComplianceDoc>(&format!(
        "SELECT {STAFF_COLUMNS}
         FROM supplier_compliance_docs d
         LEFT JOIN supplier_compliance_doc_notes n ON n.doc_id = d.id
         WHERE d.id = $1 AND d.supplier_id = $2 {lock}"
    ))
    .bind(doc_id)
    .bind(supplier_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)
}

#[utoipa::path(
    operation_id = "supplier_watch_verify_doc",
    post,
    path = "/api/v1/suppliers/{supplier_id}/compliance-docs/{doc_id}/verify",
    tag = "suppliers",
    params(("supplier_id" = Uuid, Path), ("doc_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Marked as checked by the caller", body = StaffComplianceDoc),
        (status = 403, description = "Not internal procurement staff", body = ErrorBody),
        (status = 404, description = "Unknown document", body = ErrorBody),
        (status = 409, description = "Retired", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn verify_doc(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((supplier_id, doc_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<StaffComplianceDoc>> {
    let (platform_org, mut tx) = staff_tx(&state, &p).await?;
    let before = load_staff_doc(&mut tx, supplier_id, doc_id, true).await?;
    if before.retired_at.is_some() {
        return Err(ApiError::Conflict("document_retired"));
    }
    sqlx::query(
        "UPDATE supplier_compliance_docs SET verified_by = $2, verified_at = now(),
                updated_at = now()
         WHERE id = $1",
    )
    .bind(doc_id)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    Audit {
        org_id: platform_org,
        p: &p,
    }
    .record(
        &mut tx,
        "supplier.compliance_doc_verified",
        "supplier_compliance_doc",
        doc_id,
        json!({ "supplier_id": supplier_id, "kind": before.kind,
                "reference": before.reference, "expires_on": before.expires_on }),
    )
    .await?;
    let after = load_staff_doc(&mut tx, supplier_id, doc_id, false).await?;
    tx.commit().await?;
    Ok(Json(after))
}

#[utoipa::path(
    operation_id = "supplier_watch_doc_file",
    get,
    path = "/api/v1/suppliers/{supplier_id}/compliance-docs/{doc_id}/file",
    tag = "suppliers",
    params(("supplier_id" = Uuid, Path), ("doc_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "File bytes (application/octet-stream, attachment)"),
        (status = 403, description = "Not internal procurement staff", body = ErrorBody),
        (status = 404, description = "Unknown document or no file", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn doc_file(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((supplier_id, doc_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<impl IntoResponse> {
    let (_, mut tx) = staff_tx(&state, &p).await?;
    let row: Option<(Option<String>, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT file_name, sha256 FROM supplier_compliance_docs
         WHERE id = $1 AND supplier_id = $2",
    )
    .bind(doc_id)
    .bind(supplier_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    let (Some(name), Some(hash)) = row.ok_or(ApiError::NotFound)? else {
        return Err(ApiError::NotFound);
    };
    let hash: [u8; 32] = hash
        .as_slice()
        .try_into()
        .map_err(|_| ApiError::internal(std::io::Error::other("bad stored hash")))?;
    let bytes = state.storage.get(&hash).await.map_err(ApiError::internal)?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&crate::api::attachments::disposition(&name))
            .map_err(ApiError::internal)?,
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok((headers, bytes))
}

/// A customer's member who works with suppliers or reads the agents' findings.
async fn require_supplier_view(p: &Principal, state: &AppState, org_id: Uuid) -> ApiResult<()> {
    let access = p.access(state, org_id).await?;
    if access == Access::None {
        return Err(ApiError::NotFound);
    }
    let allowed = [
        Permission::ViewAgents,
        Permission::ApproveSuppliers,
        Permission::ReviewQuotations,
        Permission::ManageRfqs,
    ]
    .into_iter()
    .any(|perm| rbac::allows(access, perm));
    if allowed {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

#[utoipa::path(
    operation_id = "supplier_watch_org_docs",
    get,
    path = "/api/v1/orgs/{org_id}/suppliers/{supplier_id}/compliance-docs",
    tag = "suppliers",
    params(("org_id" = Uuid, Path), ("supplier_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "The supplier's documents shared with this organisation", body = [CustomerComplianceDoc]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn org_docs(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, supplier_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<Vec<CustomerComplianceDoc>>> {
    require_supplier_view(&p, &state, org_id).await?;
    // Customer context, no staff scope: RLS shows only customer-visible, current documents
    // of suppliers proposed to or invited by this organisation.
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let rows = sqlx::query_as::<_, CustomerComplianceDoc>(
        "SELECT d.id, d.supplier_id, d.kind, d.reference, d.title, d.issued_on, d.expires_on,
                d.verified_at IS NOT NULL AS verified
         FROM supplier_compliance_docs d
         WHERE d.supplier_id = $1 AND d.customer_visible AND d.retired_at IS NULL
         ORDER BY d.kind, d.expires_on NULLS LAST, d.id",
    )
    .bind(supplier_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ScorecardQuery {
    /// `YYYY-MM` (Japan time). Default: the current month.
    pub month: Option<String>,
}

#[utoipa::path(
    operation_id = "supplier_watch_scorecards",
    get,
    path = "/api/v1/orgs/{org_id}/supplier-scorecards",
    tag = "suppliers",
    params(("org_id" = Uuid, Path), ScorecardQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Scorecards of the suppliers this organisation worked with in the month", body = [SupplierScorecard]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 422, description = "Invalid month", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn scorecards(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiQuery(q): ApiQuery<ScorecardQuery>,
) -> ApiResult<Json<Vec<SupplierScorecard>>> {
    require_supplier_view(&p, &state, org_id).await?;
    let today = watch::jst_today();
    let month = match q.month.as_deref() {
        None => watch::month_start(today),
        Some(m) => watch::parse_month(m).ok_or(ApiError::Validation("month"))?,
    };
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let cards = watch::scorecards(&mut tx, org_id, month, today).await?;
    tx.commit().await?;
    Ok(Json(cards))
}
