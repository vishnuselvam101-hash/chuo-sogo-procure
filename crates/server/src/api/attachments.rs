//! Files attached to procurement requests (drawings, photos, datasheets, spreadsheets).
//! Upload is JSON with base64 content (the desktop bridge speaks JSON only); download is the
//! raw bytes, always as `application/octet-stream` + `attachment` so nothing is rendered.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::IntoResponse;
use base64::Engine;
use chrono::{DateTime, Utc};
use procure_domain::rbac::{Access, Permission};
use procure_domain::workflow::RequestStatus;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::api::common::{Audit, View, lock_request, request_view, require_perm};
use crate::api::suppliers::IdResponse;
use crate::db;
use crate::error::{ApiError, ApiJson, ApiPath, ApiResult, ErrorBody};
use crate::principal::Principal;
use crate::storage::{self, MAX_ATTACHMENT_BYTES};

pub const MAX_ATTACHMENTS_PER_REQUEST: i64 = 50;
/// Base64 of the largest file plus JSON overhead.
pub const MAX_UPLOAD_BODY: usize = MAX_ATTACHMENT_BYTES / 3 * 4 + 64 * 1024;

const CLOSED: [RequestStatus; 3] = [
    RequestStatus::Cancelled,
    RequestStatus::Rejected,
    RequestStatus::Closed,
];

#[derive(Deserialize, utoipa::ToSchema)]
pub struct UploadBody {
    pub file_name: String,
    /// File content, standard base64.
    pub data_base64: String,
    /// Attach to one item of the request instead of the whole request.
    pub item_id: Option<Uuid>,
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/attachments",
    tag = "attachments",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    request_body = UploadBody,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Attachment stored", body = IdResponse),
        (status = 403, description = "Not the requester or assigned staff", body = ErrorBody),
        (status = 404, description = "Unknown or not visible", body = ErrorBody),
        (status = 409, description = "Request is closed", body = ErrorBody),
        (status = 413, description = "Larger than 10 MB"),
        (status = 422, description = "File type not allowed, content does not match, or too many files", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn upload(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<UploadBody>,
) -> ApiResult<(StatusCode, Json<IdResponse>)> {
    let (access, view) = request_view(&p, &state, org_id).await?;
    // Cheap role check before any decoding work.
    match access {
        Access::Assigned(_) => require_perm(access, Permission::OperateRequests)?,
        _ => require_perm(access, Permission::SubmitRequest)?,
    }
    let file_name =
        storage::clean_file_name(&body.file_name).ok_or(ApiError::Validation("file_name"))?;
    // Bounds memory: at most a few decoded files in flight at once.
    let _permit = state
        .upload_permits
        .acquire()
        .await
        .map_err(ApiError::internal)?;

    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let req = lock_request(&mut tx, request_id, view).await?;
    // The requester attaches to their own request; assigned staff may add documents too.
    if !matches!(access, Access::Assigned(_)) && req.requested_by != p.user_id {
        return Err(ApiError::Forbidden);
    }
    if CLOSED.contains(&req.status()?) {
        return Err(ApiError::Conflict("request_closed"));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body.data_base64.as_bytes())
        .map_err(|_| ApiError::BadRequest("invalid_base64"))?;
    if bytes.is_empty() || bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(ApiError::Validation("file_size"));
    }
    let content_type =
        storage::content_type_for(&file_name, &bytes).ok_or(ApiError::Validation("file_type"))?;
    if let Some(item) = body.item_id {
        let ok: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM request_items WHERE id = $1 AND request_id = $2)",
        )
        .bind(item)
        .bind(req.id)
        .fetch_one(&mut *tx)
        .await?;
        if !ok {
            return Err(ApiError::Validation("item_id"));
        }
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM attachments WHERE request_id = $1 AND removed_at IS NULL",
    )
    .bind(req.id)
    .fetch_one(&mut *tx)
    .await?;
    if count >= MAX_ATTACHMENTS_PER_REQUEST {
        return Err(ApiError::Validation("too_many_attachments"));
    }
    // Bytes first (content-addressed, idempotent): a failed transaction leaves at most an
    // unreferenced file, never a row pointing at nothing.
    let hash = state
        .storage
        .put(&bytes)
        .await
        .map_err(ApiError::internal)?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO attachments (id, org_id, request_id, item_id, file_name, content_type,
                                  size_bytes, sha256, uploaded_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(org_id)
    .bind(req.id)
    .bind(body.item_id)
    .bind(&file_name)
    .bind(content_type)
    .bind(i64::try_from(bytes.len()).map_err(|_| ApiError::Validation("file_size"))?)
    .bind(hash.as_slice())
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "attachment.added",
            "attachment",
            id,
            json!({ "request": req.id, "number": req.number, "file_name": file_name,
                    "size_bytes": bytes.len(), "sha256": hex::encode(hash) }),
        )
        .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(IdResponse { id })))
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct AttachmentView {
    pub id: Uuid,
    pub item_id: Option<Uuid>,
    pub file_name: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub uploaded_by: Uuid,
    pub uploaded_by_name: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/attachments",
    tag = "attachments",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Current attachments of the request", body = [AttachmentView]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or not visible", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<Vec<AttachmentView>>> {
    let (_, view) = request_view(&p, &state, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let visible: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM procurement_requests
                        WHERE id = $1 AND ($2::uuid IS NULL OR requested_by = $2))",
    )
    .bind(request_id)
    .bind(view.owner_filter())
    .fetch_one(&mut *tx)
    .await?;
    if !visible {
        return Err(ApiError::NotFound);
    }
    let rows = sqlx::query_as::<_, AttachmentView>(
        "SELECT a.id, a.item_id, a.file_name, a.content_type, a.size_bytes, a.uploaded_by,
                u.display_name AS uploaded_by_name, a.created_at
         FROM attachments a LEFT JOIN users u ON u.id = a.uploaded_by
         WHERE a.request_id = $1 AND a.removed_at IS NULL ORDER BY a.created_at, a.id",
    )
    .bind(request_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[derive(sqlx::FromRow)]
struct AttachmentRow {
    request_id: Uuid,
    file_name: String,
    sha256: Vec<u8>,
    uploaded_by: Uuid,
}

/// Loads a current attachment whose request the caller may see.
async fn visible_attachment(tx: &mut db::Tx, id: Uuid, view: View) -> ApiResult<AttachmentRow> {
    sqlx::query_as::<_, AttachmentRow>(
        "SELECT a.request_id, a.file_name, a.sha256, a.uploaded_by
         FROM attachments a JOIN procurement_requests r ON r.id = a.request_id
         WHERE a.id = $1 AND a.removed_at IS NULL AND ($2::uuid IS NULL OR r.requested_by = $2)",
    )
    .bind(id)
    .bind(view.owner_filter())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)
}

/// RFC 6266 `attachment` disposition with an ASCII fallback and the UTF-8 name.
fn disposition(file_name: &str) -> String {
    let ascii: String = file_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._- ".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded: String = file_name
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"._-".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/attachments/{attachment_id}/content",
    tag = "attachments",
    params(("org_id" = Uuid, Path), ("attachment_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "File bytes (application/octet-stream, attachment)"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown, removed or not visible", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn download(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<impl IntoResponse> {
    let (_, view) = request_view(&p, &state, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let row = visible_attachment(&mut tx, id, view).await?;
    tx.commit().await?;
    let hash: [u8; 32] = row
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
        HeaderValue::from_str(&disposition(&row.file_name)).map_err(ApiError::internal)?,
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok((headers, bytes))
}

#[utoipa::path(
    delete,
    path = "/api/v1/orgs/{org_id}/attachments/{attachment_id}",
    tag = "attachments",
    params(("org_id" = Uuid, Path), ("attachment_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Removed (hidden; the record and audit trail stay)"),
        (status = 403, description = "Only the uploader can remove it", body = ErrorBody),
        (status = 404, description = "Unknown, removed or not visible", body = ErrorBody),
        (status = 409, description = "Request is closed", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn remove(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    let (_, view) = request_view(&p, &state, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let row = visible_attachment(&mut tx, id, view).await?;
    let req = lock_request(&mut tx, row.request_id, View::All).await?;
    if row.uploaded_by != p.user_id {
        return Err(ApiError::Forbidden);
    }
    if CLOSED.contains(&req.status()?) {
        return Err(ApiError::Conflict("request_closed"));
    }
    sqlx::query(
        "UPDATE attachments SET removed_at = now(), removed_by = $2
         WHERE id = $1 AND removed_at IS NULL",
    )
    .bind(id)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "attachment.removed",
            "attachment",
            id,
            json!({ "request": req.id, "number": req.number, "file_name": row.file_name }),
        )
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::disposition;

    #[test]
    fn disposition_is_always_attachment_and_escapes_names() {
        let d = disposition("見積 \"x\".pdf");
        assert!(d.starts_with("attachment; filename=\"__ _x_.pdf\"; filename*=UTF-8''"));
        assert!(d.contains("%E8%A6%8B"));
        assert!(!d.contains('\n'));
    }
}
