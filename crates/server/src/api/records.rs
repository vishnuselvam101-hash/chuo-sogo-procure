//! Electronic transaction records (電子帳簿保存法): what suppliers sent is kept unchanged
//! (`supplier_documents`, add-only, with its SHA-256), and every quotation, order and invoice
//! of the factory can be found by date, amount and counterparty, as the law requires of a
//! company of the factory's size.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::IntoResponse;
use chrono::{DateTime, NaiveDate, Utc};
use procure_domain::rbac::{self, Access, Permission};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::api::common;
use crate::db;
use crate::error::{ApiError, ApiPath, ApiQuery, ApiResult, ErrorBody};
use crate::principal::Principal;
use crate::storage;

/// The stored type of a kept supplier file: as received, never rendered by the client.
fn document_type(file_name: &str, bytes: &[u8]) -> &'static str {
    let ext = std::path::Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "eml" => "message/rfc822",
        "tif" | "tiff" => "image/tiff",
        "xls" => "application/vnd.ms-excel",
        _ => storage::content_type_for(file_name, bytes).unwrap_or("application/octet-stream"),
    }
}

/// How a kept file arrived.
#[derive(Clone, Copy)]
pub enum Channel {
    Upload,
    Email,
    Fax,
}

impl Channel {
    const fn code(self) -> &'static str {
        match self {
            Self::Upload => "upload",
            Self::Email => "email",
            Self::Fax => "fax",
        }
    }
}

pub struct KeptFile<'a> {
    pub rfq_id: Uuid,
    pub supplier_id: Option<Uuid>,
    pub channel: Channel,
    pub file_name: &'a str,
    pub bytes: &'a [u8],
    pub received_at: Option<DateTime<Utc>>,
}

/// Keeps a supplier's file as received and records it in the audit trail. The bytes are
/// stored first (content-addressed), so a failed transaction leaves no row without a file.
pub(crate) async fn keep_document(
    state: &AppState,
    tx: &mut db::Tx,
    org_id: Uuid,
    p: Option<&Principal>,
    f: &KeptFile<'_>,
) -> ApiResult<Uuid> {
    if f.bytes.is_empty() || f.bytes.len() > storage::MAX_ATTACHMENT_BYTES {
        return Err(ApiError::Validation("file_size"));
    }
    let file_name = storage::clean_file_name(f.file_name).unwrap_or_else(|| "reply".to_string());
    let hash = state
        .storage
        .put(f.bytes)
        .await
        .map_err(ApiError::internal)?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO supplier_documents (id, org_id, rfq_id, supplier_id, channel, file_name,
                                         content_type, size_bytes, sha256, received_at,
                                         uploaded_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, coalesce($10, now()), $11)",
    )
    .bind(id)
    .bind(org_id)
    .bind(f.rfq_id)
    .bind(f.supplier_id)
    .bind(f.channel.code())
    .bind(&file_name)
    .bind(document_type(&file_name, f.bytes))
    .bind(i64::try_from(f.bytes.len()).map_err(|_| ApiError::Validation("file_size"))?)
    .bind(hash.as_slice())
    .bind(f.received_at)
    .bind(p.map(|p| p.user_id))
    .execute(&mut **tx)
    .await?;
    let detail = json!({ "rfq": f.rfq_id, "file_name": file_name, "channel": f.channel.code(),
                         "size_bytes": f.bytes.len(), "sha256": hex::encode(hash) });
    crate::audit::record(
        tx,
        crate::audit::Event {
            org_id,
            actor: p.map(|p| p.user_id),
            action: "supplier_document.kept",
            entity_type: "supplier_document",
            entity_id: Some(id.to_string()),
            data: detail,
            request_id: p.and_then(|p| p.request_id.as_deref()),
        },
    )
    .await?;
    Ok(id)
}

async fn require_read(state: &AppState, p: &Principal, org_id: Uuid) -> ApiResult<Access> {
    let access = p.access(state, org_id).await?;
    if access == Access::None {
        return Err(ApiError::NotFound);
    }
    if rbac::allows(access, Permission::ViewFinance)
        || rbac::allows(access, Permission::ViewAllRequests)
    {
        Ok(access)
    } else {
        Err(ApiError::Forbidden)
    }
}

pub const RECORD_KINDS: [&str; 4] = ["quotation", "order", "supplier_invoice", "fee_invoice"];

#[derive(Deserialize, utoipa::IntoParams)]
pub struct RecordQuery {
    /// Dates in the organisation's time zone, inclusive.
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    /// Part of the counterparty's name (case-insensitive).
    pub counterparty: Option<String>,
    /// Amount in minor units, inclusive.
    pub min_minor: Option<i64>,
    pub max_minor: Option<i64>,
    /// quotation, order, supplier_invoice or fee_invoice.
    pub kind: Option<String>,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct RecordRow {
    pub kind: String,
    pub id: Uuid,
    /// Transaction date in the organisation's time zone.
    pub date: NaiveDate,
    pub amount_minor: i64,
    pub currency: String,
    pub counterparty: String,
    /// The counterparty's or the factory's document number.
    pub reference: String,
    pub request_number: Option<String>,
    /// The file the record was read from, kept as received.
    pub document_id: Option<Uuid>,
    pub document_name: Option<String>,
    pub document_sha256: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct RecordList {
    pub rows: Vec<RecordRow>,
    /// More rows matched than returned (narrow the search).
    pub truncated: bool,
}

const MAX_ROWS: i64 = 1000;

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/records",
    tag = "records",
    params(("org_id" = Uuid, Path), RecordQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Transaction records matching every given condition, newest first", body = RecordList),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation", body = ErrorBody),
        (status = 422, description = "Unknown kind or inverted range", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn search_records(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiQuery(q): ApiQuery<RecordQuery>,
) -> ApiResult<Json<RecordList>> {
    require_read(&state, &p, org_id).await?;
    if let Some(k) = &q.kind
        && !RECORD_KINDS.contains(&k.as_str())
    {
        return Err(ApiError::Validation("kind"));
    }
    if let (Some(a), Some(b)) = (q.from, q.to)
        && a > b
    {
        return Err(ApiError::Validation("to"));
    }
    if let (Some(a), Some(b)) = (q.min_minor, q.max_minor)
        && a > b
    {
        return Err(ApiError::Validation("max_minor"));
    }
    let who = q
        .counterparty
        .as_deref()
        .map(|s| common::text(s, 100, false, "counterparty"))
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
    // Supplier names come through the factory's own RFQs (suppliers RLS), no staff scope.
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let rows = sqlx::query_as::<_, RecordRow>(
        "WITH tz AS (SELECT timezone FROM organisations WHERE id = $1),
         r AS (
           SELECT 'quotation'::text AS kind, q.id, (q.received_at AT TIME ZONE (SELECT timezone FROM tz))::date AS date,
                  -- numeric, clamped: one absurd quotation must not break every search.
                  LEAST(q.shipping_minor::numeric
                        + coalesce((SELECT sum(l.unit_price_minor::numeric * l.quantity)
                                    FROM quotation_lines l WHERE l.quotation_id = q.id), 0),
                        9223372036854775807)::bigint AS amount_minor,
                  q.currency, s.name AS counterparty,
                  CASE WHEN q.supplier_reference = '' THEN f.number
                       ELSE q.supplier_reference || ' (' || f.number || ')' END AS reference,
                  pr.number AS request_number,
                  d.id AS document_id, d.file_name AS document_name,
                  encode(d.sha256, 'hex') AS document_sha256
           FROM quotations q
           JOIN rfq_recipients rr ON rr.id = q.rfq_recipient_id
           JOIN suppliers s ON s.id = rr.supplier_id
           JOIN rfqs f ON f.id = q.rfq_id
           JOIN procurement_requests pr ON pr.id = f.request_id
           LEFT JOIN supplier_documents d ON d.id = q.source_document_id
           UNION ALL
           SELECT 'order', o.id, (o.ordered_at AT TIME ZONE (SELECT timezone FROM tz))::date, o.total_minor,
                  o.currency, s.name, o.po_number, pr.number, NULL, NULL, NULL
           FROM purchase_orders o
           JOIN suppliers s ON s.id = o.supplier_id
           JOIN procurement_requests pr ON pr.id = o.request_id
           UNION ALL
           SELECT 'supplier_invoice', i.id, i.invoice_date, i.total_minor, i.currency, s.name,
                  i.supplier_invoice_number, pr.number, NULL, NULL, NULL
           FROM invoices i
           JOIN suppliers s ON s.id = i.supplier_id
           JOIN purchase_orders o ON o.id = i.order_id
           JOIN procurement_requests pr ON pr.id = o.request_id
           UNION ALL
           SELECT 'fee_invoice', ii.id, ii.issue_date, ii.total_minor, ii.currency,
                  'Chuo-Sogo', ii.number, NULL, NULL, NULL, NULL
           FROM issued_invoices ii
           WHERE ii.org_id = $1 AND ii.status <> 'draft' AND ii.issue_date IS NOT NULL
         )
         SELECT * FROM r
         WHERE ($2::date IS NULL OR date >= $2) AND ($3::date IS NULL OR date <= $3)
           AND ($4::text IS NULL OR counterparty ILIKE $4)
           AND ($5::bigint IS NULL OR amount_minor >= $5)
           AND ($6::bigint IS NULL OR amount_minor <= $6)
           AND ($7::text IS NULL OR kind = $7)
         ORDER BY date DESC, kind, id DESC
         LIMIT $8",
    )
    .bind(org_id)
    .bind(q.from)
    .bind(q.to)
    .bind(who)
    .bind(q.min_minor)
    .bind(q.max_minor)
    .bind(q.kind)
    .bind(MAX_ROWS + 1)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let truncated = rows.len() > usize::try_from(MAX_ROWS).unwrap_or(usize::MAX);
    let mut rows = rows;
    rows.truncate(usize::try_from(MAX_ROWS).unwrap_or(usize::MAX));
    Ok(Json(RecordList { rows, truncated }))
}

#[derive(sqlx::FromRow)]
struct DocRow {
    file_name: String,
    sha256: Vec<u8>,
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/supplier-documents/{document_id}/content",
    tag = "records",
    params(("org_id" = Uuid, Path), ("document_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "The file as the supplier sent it (application/octet-stream, attachment)"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown document", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn download_document(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, document_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<impl IntoResponse> {
    require_read(&state, &p, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let d = sqlx::query_as::<_, DocRow>(
        "SELECT file_name, sha256 FROM supplier_documents WHERE id = $1",
    )
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    tx.commit().await?;
    let hash: [u8; 32] = d
        .sha256
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
        HeaderValue::from_str(&crate::api::attachments::disposition(&d.file_name))
            .map_err(ApiError::internal)?,
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok((headers, bytes))
}
