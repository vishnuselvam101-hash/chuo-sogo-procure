//! Integrations (docs/agents/INTEGRATIONS.md): the factory's purchase orders, goods receipts
//! and supplier invoices from its ERP / accounting exports, and agent results back out as CSV.
//!
//! Upload → preview: the file is read into a batch (`import_batches` + staged `import_rows`);
//! no business table changes. The `import` agent cross-checks the batch against the orders and
//! proposes applying it. Apply (by a person, directly or by approving that proposal) writes
//! every record through the same functions as manual entry (`orders::record_*_tx`), each
//! audited, all or nothing.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use procure_domain::agents::Trigger;
use procure_domain::connector::{self as conn, ImportKind, KnownLine, Profile};
use procure_domain::rbac::{self, Access, Permission};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Digest;
use uuid::Uuid;

use crate::AppState;
use crate::agents::{self, queue};
use crate::api::common::{self, Audit, View, require_perm, side};
use crate::api::orders::{self, DeliveryInput, DeliveryLineInput, InvoiceInput, InvoiceLineInput};
use crate::db;
use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ApiResult, ErrorBody};
use crate::integrations::{
    Connector, ConnectorError, Export, FileConnector, ProfileConfig, PullRequest, Staged,
};
use crate::principal::Principal;

/// Row errors kept on a batch (the rest are counted).
const MAX_STORED_ERRORS: usize = 500;
/// Staged records shown with an upload's preview.
const PREVIEW_RECORDS: usize = 100;
/// Rows in one agent-results file.
const MAX_EXPORT_ROWS: i64 = 50_000;

/// The permission recording this kind of record by hand needs.
pub fn kind_permission(kind: ImportKind) -> Permission {
    match kind {
        ImportKind::Po => Permission::RecordOrders,
        ImportKind::Receipt => Permission::ReceiveGoods,
        ImportKind::Invoice => Permission::ManageInvoices,
    }
}

fn kind_of(code: &str) -> ApiResult<ImportKind> {
    ImportKind::from_code(code).ok_or(ApiError::Validation("kind"))
}

/// Any import permission (to see profiles and batches).
async fn require_any(state: &AppState, p: &Principal, org_id: Uuid) -> ApiResult<Access> {
    let access = p.access(state, org_id).await?;
    if access == Access::None {
        return Err(ApiError::NotFound);
    }
    if ImportKind::ALL
        .into_iter()
        .any(|k| rbac::allows(access, kind_permission(k)))
    {
        Ok(access)
    } else {
        Err(ApiError::Forbidden)
    }
}

fn connector_error(e: ConnectorError) -> ApiError {
    match e {
        ConnectorError::Intake(i) => crate::api::history::intake_error(i),
        other => ApiError::Validation(other.code()),
    }
}

// ---------------------------------------------------------------------------------------
// Profiles

#[derive(Serialize, utoipa::ToSchema)]
pub struct ProfileView {
    pub name: String,
    /// Built-in generic preset (read-only).
    pub builtin: bool,
    pub config: ProfileConfig,
    pub updated_at: Option<DateTime<Utc>>,
}

#[utoipa::path(
    operation_id = "integrations_list_profiles",
    get,
    path = "/api/v1/orgs/{org_id}/integrations/profiles",
    tag = "integrations",
    params(("org_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Built-in presets, then the organisation's saved profiles", body = [ProfileView]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list_profiles(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
) -> ApiResult<Json<Vec<ProfileView>>> {
    require_any(&state, &p, org_id).await?;
    let mut out: Vec<ProfileView> = conn::presets()
        .iter()
        .map(|pr| ProfileView {
            name: pr.name.clone(),
            builtin: true,
            config: ProfileConfig::from_profile(pr),
            updated_at: None,
        })
        .collect();
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let saved: Vec<(String, Value, DateTime<Utc>)> = sqlx::query_as(
        "SELECT name, config, updated_at FROM integration_profiles WHERE org_id = $1
         ORDER BY name",
    )
    .bind(org_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    for (name, config, updated_at) in saved {
        let config: ProfileConfig = serde_json::from_value(config).map_err(ApiError::internal)?;
        out.push(ProfileView {
            name,
            builtin: false,
            config,
            updated_at: Some(updated_at),
        });
    }
    Ok(Json(out))
}

#[utoipa::path(
    operation_id = "integrations_put_profile",
    put,
    path = "/api/v1/orgs/{org_id}/integrations/profiles/{name}",
    tag = "integrations",
    params(("org_id" = Uuid, Path), ("name" = String, Path)),
    request_body = ProfileConfig,
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Profile saved", body = ProfileView),
        (status = 403, description = "Not permitted for this kind of record", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 409, description = "The name of a built-in preset (preset_name)", body = ErrorBody),
        (status = 422, description = "Invalid profile", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn put_profile(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, name)): ApiPath<(Uuid, String)>,
    ApiJson(body): ApiJson<ProfileConfig>,
) -> ApiResult<Json<ProfileView>> {
    let profile = body.to_profile(&name).map_err(ApiError::Validation)?;
    p.require(&state, org_id, kind_permission(profile.kind))
        .await?;
    if conn::preset(&name).is_some() {
        return Err(ApiError::Conflict("preset_name"));
    }
    let config = ProfileConfig::from_profile(&profile);
    let value = serde_json::to_value(&config).map_err(ApiError::internal)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    // Changing a profile of another kind needs that kind's permission too.
    let existing: Option<String> = sqlx::query_scalar(
        "SELECT kind FROM integration_profiles WHERE org_id = $1 AND name = $2 FOR UPDATE",
    )
    .bind(org_id)
    .bind(&name)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(k) = &existing {
        let access = p.access(&state, org_id).await?;
        require_perm(access, kind_permission(kind_of(k)?))?;
    }
    let (id, updated_at): (Uuid, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO integration_profiles (id, org_id, name, kind, config, created_by, updated_by)
         VALUES ($1, $2, $3, $4, $5, $6, $6)
         ON CONFLICT (org_id, name) DO UPDATE SET kind = EXCLUDED.kind,
             config = EXCLUDED.config, updated_by = EXCLUDED.updated_by, updated_at = now()
         RETURNING id, updated_at",
    )
    .bind(Uuid::now_v7())
    .bind(org_id)
    .bind(&name)
    .bind(profile.kind.code())
    .bind(&value)
    .bind(p.user_id)
    .fetch_one(&mut *tx)
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "integration.profile_saved",
            "integration_profile",
            id,
            json!({ "name": name, "kind": profile.kind.code(), "config": value }),
        )
        .await?;
    tx.commit().await?;
    Ok(Json(ProfileView {
        name,
        builtin: false,
        config,
        updated_at: Some(updated_at),
    }))
}

#[utoipa::path(
    operation_id = "integrations_delete_profile",
    delete,
    path = "/api/v1/orgs/{org_id}/integrations/profiles/{name}",
    tag = "integrations",
    params(("org_id" = Uuid, Path), ("name" = String, Path)),
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Profile removed (past batches keep the settings they used)"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation or profile", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn delete_profile(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, name)): ApiPath<(Uuid, String)>,
) -> ApiResult<StatusCode> {
    let access = require_any(&state, &p, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let (id, kind): (Uuid, String) = sqlx::query_as(
        "SELECT id, kind FROM integration_profiles WHERE org_id = $1 AND name = $2 FOR UPDATE",
    )
    .bind(org_id)
    .bind(&name)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    require_perm(access, kind_permission(kind_of(&kind)?))?;
    sqlx::query("DELETE FROM integration_profiles WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "integration.profile_deleted",
            "integration_profile",
            id,
            json!({ "name": name, "kind": kind }),
        )
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------------------
// Batches

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ImportUpload {
    /// `po` | `receipt` | `invoice`
    pub kind: String,
    /// `.csv`, `.tsv`, `.txt` (UTF-8 or Shift_JIS) or `.xlsx`.
    pub file_name: String,
    /// File content, standard base64 (at most 10 MB).
    pub data_base64: String,
    /// A built-in preset or a saved profile (default: headers detected).
    pub profile: Option<String>,
    /// Settings for this file only (overrides `profile`'s).
    pub config: Option<ProfileConfig>,
    /// For records without a currency column (default JPY).
    pub currency: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, utoipa::ToSchema)]
pub struct RowErrorView {
    /// File row, 1-based as a spreadsheet numbers it (0 = the whole file).
    pub row: usize,
    pub field: Option<String>,
    pub code: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ImportBatchView {
    pub id: Uuid,
    pub kind: String,
    pub file_name: String,
    pub file_sha256: String,
    pub profile: String,
    pub currency: String,
    /// Field code of each column (`null` = ignored).
    pub mapping: Vec<Option<String>>,
    /// `previewed` | `applied` | `failed`
    pub status: String,
    /// Why it failed: `nothing_to_import` or `discarded`.
    pub failure_code: Option<String>,
    pub data_rows: i32,
    pub records: i32,
    pub error_rows: i32,
    pub errors: Vec<RowErrorView>,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub decided_by: Option<Uuid>,
    pub decided_at: Option<DateTime<Utc>>,
    /// Staged records (an upload's preview shows the first 100).
    pub staged: Vec<Staged>,
}

#[derive(sqlx::FromRow)]
struct BatchRow {
    id: Uuid,
    kind: String,
    file_name: String,
    file_sha256: String,
    profile: String,
    currency: String,
    mapping: Value,
    status: String,
    failure_code: Option<String>,
    data_rows: i32,
    records: i32,
    error_rows: i32,
    errors: Value,
    created_by: Uuid,
    created_at: DateTime<Utc>,
    decided_by: Option<Uuid>,
    decided_at: Option<DateTime<Utc>>,
}

const BATCH_COLUMNS: &str = "id, kind, file_name, file_sha256, profile, currency, mapping, \
     status, failure_code, data_rows, records, error_rows, errors, created_by, created_at, \
     decided_by, decided_at";

impl BatchRow {
    fn view(self, staged: Vec<Staged>) -> ApiResult<ImportBatchView> {
        Ok(ImportBatchView {
            id: self.id,
            kind: self.kind,
            file_name: self.file_name,
            file_sha256: self.file_sha256,
            profile: self.profile,
            currency: self.currency,
            mapping: serde_json::from_value(self.mapping).map_err(ApiError::internal)?,
            status: self.status,
            failure_code: self.failure_code,
            data_rows: self.data_rows,
            records: self.records,
            error_rows: self.error_rows,
            errors: serde_json::from_value(self.errors).map_err(ApiError::internal)?,
            created_by: self.created_by,
            created_at: self.created_at,
            decided_by: self.decided_by,
            decided_at: self.decided_at,
            staged,
        })
    }
}

async fn load_batch(tx: &mut db::Tx, org_id: Uuid, id: Uuid, lock: bool) -> ApiResult<BatchRow> {
    sqlx::query_as::<_, BatchRow>(&format!(
        "SELECT {BATCH_COLUMNS} FROM import_batches WHERE id = $1 AND org_id = $2 {}",
        if lock { "FOR UPDATE" } else { "" }
    ))
    .bind(id)
    .bind(org_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)
}

/// Staged records of a batch, in file order: (seq, record).
pub(crate) async fn staged_of(
    tx: &mut db::Tx,
    batch_id: Uuid,
    limit: i64,
) -> ApiResult<Vec<(i32, Staged)>> {
    let rows: Vec<(i32, Value)> = sqlx::query_as(
        "SELECT seq, record FROM import_rows WHERE batch_id = $1 ORDER BY seq LIMIT $2",
    )
    .bind(batch_id)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|(seq, v)| Ok((seq, serde_json::from_value(v).map_err(ApiError::internal)?)))
        .collect()
}

/// The profile an upload uses: inline settings, else a saved profile or preset, else detect.
async fn resolve_profile(
    tx: &mut db::Tx,
    org_id: Uuid,
    kind: ImportKind,
    name: Option<&str>,
    config: Option<&ProfileConfig>,
) -> ApiResult<Profile> {
    let name = name.unwrap_or(match config {
        Some(_) => "custom",
        None => "auto",
    });
    if !conn::valid_profile_name(name) {
        return Err(ApiError::Validation("profile"));
    }
    let profile = if let Some(c) = config {
        c.to_profile(name).map_err(ApiError::Validation)?
    } else if let Some(pr) = conn::preset(name) {
        pr
    } else if name == "auto" {
        Profile::detect("auto", kind)
    } else {
        let saved: Value = sqlx::query_scalar(
            "SELECT config FROM integration_profiles WHERE org_id = $1 AND name = $2",
        )
        .bind(org_id)
        .bind(name)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::Validation("profile"))?;
        let c: ProfileConfig = serde_json::from_value(saved).map_err(ApiError::internal)?;
        c.to_profile(name).map_err(ApiError::Validation)?
    };
    if profile.kind != kind {
        return Err(ApiError::Validation("profile_kind"));
    }
    Ok(profile)
}

#[utoipa::path(
    operation_id = "integrations_upload",
    post,
    path = "/api/v1/orgs/{org_id}/integrations/imports",
    tag = "integrations",
    params(("org_id" = Uuid, Path)),
    request_body = ImportUpload,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "File read into a batch (preview); nothing else changed. Rows that could not be read are listed", body = ImportBatchView),
        (status = 403, description = "Not permitted for this kind of record", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 409, description = "The same file was already imported (duplicate_file)", body = ErrorBody),
        (status = 422, description = "Unsupported, empty or oversized file, wrong encoding, no header, or invalid profile", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn upload(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<ImportUpload>,
) -> ApiResult<(StatusCode, Json<ImportBatchView>)> {
    use base64::Engine;
    let kind = kind_of(&body.kind)?;
    let access = p.require(&state, org_id, kind_permission(kind)).await?;
    side(access)?;
    let file_name = common::required(&body.file_name, 255, false, "file_name")?;
    let currency = common::currency(body.currency.as_deref().unwrap_or("JPY"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body.data_base64.as_bytes())
        .map_err(|_| ApiError::BadRequest("invalid_base64"))?;
    let sha = hex::encode(sha2::Sha256::digest(&bytes));
    let size = i32::try_from(bytes.len()).map_err(|_| ApiError::Validation("file_size"))?;
    let profile = {
        let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
        let pr = resolve_profile(
            &mut tx,
            org_id,
            kind,
            body.profile.as_deref(),
            body.config.as_ref(),
        )
        .await?;
        tx.commit().await?;
        pr
    };
    let pulled = {
        let _permit = state
            .upload_permits
            .acquire()
            .await
            .map_err(ApiError::internal)?;
        FileConnector
            .pull(
                &PullRequest {
                    file_name: file_name.clone(),
                    bytes,
                    currency,
                },
                &profile,
            )
            .await
            .map_err(connector_error)?
    };
    let mut error_rows: Vec<usize> = pulled.errors.iter().map(|e| e.row).collect();
    error_rows.dedup();
    let errors: Vec<RowErrorView> = pulled
        .errors
        .iter()
        .take(MAX_STORED_ERRORS)
        .map(|e| RowErrorView {
            row: e.row,
            field: e.field.map(|f| f.code().to_string()),
            code: e.code.code().to_string(),
        })
        .collect();
    let mapping: Vec<Option<&str>> = pulled.mapping.iter().map(|m| m.map(|f| f.code())).collect();
    let staged: Vec<Staged> = pulled.records.iter().map(Staged::from_record).collect();
    let failed = staged.is_empty();
    let id = Uuid::now_v7();
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let duplicate: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM import_batches
                        WHERE org_id = $1 AND file_sha256 = $2 AND status <> 'failed')",
    )
    .bind(org_id)
    .bind(&sha)
    .fetch_one(&mut *tx)
    .await?;
    if duplicate {
        return Err(ApiError::Conflict("duplicate_file"));
    }
    sqlx::query(
        "INSERT INTO import_batches (id, org_id, kind, file_name, file_sha256, file_size, profile,
                                     profile_config, currency, mapping, status, failure_code,
                                     data_rows, records, error_rows, errors, created_by,
                                     decided_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17,
                 CASE WHEN $11 = 'failed' THEN now() END)",
    )
    .bind(id)
    .bind(org_id)
    .bind(kind.code())
    .bind(&file_name)
    .bind(&sha)
    .bind(size)
    .bind(&profile.name)
    .bind(serde_json::to_value(ProfileConfig::from_profile(&profile)).map_err(ApiError::internal)?)
    .bind(currency.code())
    .bind(json!(mapping))
    .bind(if failed { "failed" } else { "previewed" })
    .bind(failed.then_some("nothing_to_import"))
    .bind(i32::try_from(pulled.data_rows).unwrap_or(i32::MAX))
    .bind(i32::try_from(staged.len()).unwrap_or(i32::MAX))
    .bind(i32::try_from(error_rows.len()).unwrap_or(i32::MAX))
    .bind(serde_json::to_value(&errors).map_err(ApiError::internal)?)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| match ApiError::from(e) {
        ApiError::Conflict(_) => ApiError::Conflict("duplicate_file"),
        other => other,
    })?;
    for (seq, s) in staged.iter().enumerate() {
        sqlx::query(
            "INSERT INTO import_rows (id, org_id, batch_id, seq, source_rows, po_number, record)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(Uuid::now_v7())
        .bind(org_id)
        .bind(id)
        .bind(i32::try_from(seq).map_err(ApiError::internal)?)
        .bind(&s.rows)
        .bind(&s.po_number)
        .bind(serde_json::to_value(s).map_err(ApiError::internal)?)
        .execute(&mut *tx)
        .await?;
    }
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "import.previewed",
            "import_batch",
            id,
            json!({ "kind": kind.code(), "file_name": file_name, "sha256": sha,
                    "profile": profile.name, "records": staged.len(),
                    "data_rows": pulled.data_rows, "error_rows": error_rows.len(),
                    "status": if failed { "failed" } else { "previewed" } }),
        )
        .await?;
    if !failed {
        // The import agent checks the batch against the orders and proposes applying it
        // (nothing is queued when agents are off: a person applies it directly).
        let key = format!("import:batch:{id}");
        queue::enqueue_by(
            &mut tx,
            org_id,
            crate::agents::import::NAME,
            Trigger::Event,
            Some(("import_batch", id)),
            json!({}),
            Some(&key),
            Some(p.user_id),
        )
        .await?;
    }
    let row = load_batch(&mut tx, org_id, id, false).await?;
    tx.commit().await?;
    let preview = staged.into_iter().take(PREVIEW_RECORDS).collect();
    Ok((StatusCode::CREATED, Json(row.view(preview)?)))
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct BatchesQuery {
    /// `po` | `receipt` | `invoice`
    pub kind: Option<String>,
    /// At most 200 (default 50), newest first.
    pub limit: Option<i64>,
}

#[utoipa::path(
    operation_id = "integrations_list_imports",
    get,
    path = "/api/v1/orgs/{org_id}/integrations/imports",
    tag = "integrations",
    params(("org_id" = Uuid, Path), BatchesQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Batches, newest first (without staged records)", body = [ImportBatchView]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 422, description = "Invalid kind", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list_imports(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiQuery(q): ApiQuery<BatchesQuery>,
) -> ApiResult<Json<Vec<ImportBatchView>>> {
    require_any(&state, &p, org_id).await?;
    let kind = q.kind.as_deref().map(kind_of).transpose()?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let rows = sqlx::query_as::<_, BatchRow>(&format!(
        "SELECT {BATCH_COLUMNS} FROM import_batches
         WHERE org_id = $1 AND ($2::text IS NULL OR kind = $2)
         ORDER BY created_at DESC, id DESC LIMIT $3"
    ))
    .bind(org_id)
    .bind(kind.map(ImportKind::code))
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| r.view(Vec::new()))
            .collect::<ApiResult<_>>()?,
    ))
}

#[utoipa::path(
    operation_id = "integrations_get_import",
    get,
    path = "/api/v1/orgs/{org_id}/integrations/imports/{batch_id}",
    tag = "integrations",
    params(("org_id" = Uuid, Path), ("batch_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "The batch with every staged record", body = ImportBatchView),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation or batch", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn get_import(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, batch_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<ImportBatchView>> {
    require_any(&state, &p, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let row = load_batch(&mut tx, org_id, batch_id, false).await?;
    let staged = staged_of(&mut tx, batch_id, i64::MAX).await?;
    tx.commit().await?;
    Ok(Json(
        row.view(staged.into_iter().map(|(_, s)| s).collect())?,
    ))
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct AppliedRecord {
    pub seq: i32,
    pub po_number: String,
    /// `purchase_order` | `delivery` | `invoice`
    pub entity_type: String,
    pub entity_id: Uuid,
    /// Invoices: `matched` or `disputed` (the same three-way match as manual entry).
    pub match_status: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ApplyResult {
    pub batch: ImportBatchView,
    pub applied: Vec<AppliedRecord>,
}

#[utoipa::path(
    operation_id = "integrations_apply",
    post,
    path = "/api/v1/orgs/{org_id}/integrations/imports/{batch_id}/apply",
    tag = "integrations",
    params(("org_id" = Uuid, Path), ("batch_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Every record written through the manual-entry path, all or nothing", body = ApplyResult),
        (status = 403, description = "Not permitted for this kind of record", body = ErrorBody),
        (status = 404, description = "Unknown organisation or batch", body = ErrorBody),
        (status = 409, description = "Already applied or failed, or a record's order is not in the right state", body = ErrorBody),
        (status = 422, description = "A record refers to an unknown request, order or item, or is invalid", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn apply(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, batch_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<ApplyResult>> {
    let access = require_any(&state, &p, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let applied = apply_batch(&mut tx, &p, access, org_id, batch_id).await?;
    agents::resolve_elsewhere(&mut tx, org_id, &format!("import:batch:{batch_id}")).await?;
    let row = load_batch(&mut tx, org_id, batch_id, false).await?;
    tx.commit().await?;
    Ok(Json(ApplyResult {
        batch: row.view(Vec::new())?,
        applied,
    }))
}

#[utoipa::path(
    operation_id = "integrations_discard",
    post,
    path = "/api/v1/orgs/{org_id}/integrations/imports/{batch_id}/discard",
    tag = "integrations",
    params(("org_id" = Uuid, Path), ("batch_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Batch discarded (failed); the same file may be uploaded again", body = ImportBatchView),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation or batch", body = ErrorBody),
        (status = 409, description = "Already applied or failed", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn discard(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, batch_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<ImportBatchView>> {
    let access = require_any(&state, &p, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let batch = load_batch(&mut tx, org_id, batch_id, true).await?;
    require_perm(access, kind_permission(kind_of(&batch.kind)?))?;
    if batch.status != "previewed" {
        return Err(ApiError::Conflict("import_not_previewed"));
    }
    sqlx::query(
        "UPDATE import_batches SET status = 'failed', failure_code = 'discarded',
                decided_by = $2, decided_at = now()
         WHERE id = $1",
    )
    .bind(batch_id)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "import.discarded",
            "import_batch",
            batch_id,
            json!({ "kind": batch.kind, "file_name": batch.file_name }),
        )
        .await?;
    agents::resolve_elsewhere(&mut tx, org_id, &format!("import:batch:{batch_id}")).await?;
    let row = load_batch(&mut tx, org_id, batch_id, false).await?;
    tx.commit().await?;
    Ok(Json(row.view(Vec::new())?))
}

/// (order item, request line, description, offered / requested / verified part numbers,
/// quantity, unit price).
type OrderLineRow = (Uuid, i32, String, String, String, String, i32, i64);

/// The order lines a record's lines may name: (order item id, matching data).
pub(crate) async fn known_lines(
    tx: &mut db::Tx,
    order_id: Uuid,
) -> ApiResult<Vec<(Uuid, KnownLine, i32, i64)>> {
    let rows: Vec<OrderLineRow> = sqlx::query_as(
        "SELECT oi.id, ri.line_no, ri.description, oi.offered_mpn, ri.mpn,
                coalesce(ri.verified_mpn, ''), oi.quantity, oi.unit_price_minor
         FROM order_items oi JOIN request_items ri ON ri.id = oi.request_item_id
         WHERE oi.order_id = $1 ORDER BY ri.line_no",
    )
    .bind(order_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, line_no, description, offered, mpn, verified, qty, price)| {
                let mpns = [offered, verified, mpn]
                    .into_iter()
                    .filter(|m| !m.trim().is_empty())
                    .collect();
                (
                    id,
                    KnownLine {
                        line_no: u32::try_from(line_no).unwrap_or(0),
                        mpns,
                        description,
                    },
                    qty,
                    price,
                )
            },
        )
        .collect())
}

pub(crate) fn record_line(l: &crate::integrations::StagedLine) -> conn::RecordLine {
    conn::RecordLine {
        row: usize::try_from(l.row).unwrap_or(0),
        line_no: l.line_no,
        mpn: l.mpn.clone(),
        description: l.description.clone(),
        quantity: l.quantity,
        unit_price_minor: l.unit_price_minor,
    }
}

/// The order with this PO number (visible to the caller), if any.
pub(crate) async fn order_by_po(tx: &mut db::Tx, po_number: &str) -> ApiResult<Option<Uuid>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM purchase_orders WHERE po_number = $1")
            .bind(po_number)
            .fetch_optional(&mut **tx)
            .await?,
    )
}

/// Applies a previewed batch in the caller's transaction (in `org_id`'s context), as `p`:
/// every staged record goes through the same function as its manual entry (with its checks,
/// workflow transitions, three-way match and audit events), plus one `import.record_applied`
/// event per record. The first refusal fails the whole batch (the transaction is not
/// committed). Needs the permission manual entry of the batch's kind needs.
pub(crate) async fn apply_batch(
    tx: &mut db::Tx,
    p: &Principal,
    access: Access,
    org_id: Uuid,
    batch_id: Uuid,
) -> ApiResult<Vec<AppliedRecord>> {
    let batch = load_batch(tx, org_id, batch_id, true).await?;
    let kind = kind_of(&batch.kind)?;
    require_perm(access, kind_permission(kind))?;
    let side = side(access)?;
    if batch.status != "previewed" {
        return Err(ApiError::Conflict("import_not_previewed"));
    }
    let view = if rbac::allows(access, Permission::ViewAllRequests) {
        View::All
    } else if rbac::allows(access, Permission::ViewOwnRequests) {
        View::Own(p.user_id)
    } else {
        return Err(ApiError::Forbidden);
    };
    let audit = Audit { org_id, p };
    let mut applied = Vec::new();
    for (seq, s) in staged_of(tx, batch_id, i64::MAX).await? {
        let (entity_type, entity_id, match_status) = match kind {
            ImportKind::Po => {
                let request_id: Uuid =
                    sqlx::query_scalar("SELECT id FROM procurement_requests WHERE number = $1")
                        .bind(&s.request_number)
                        .fetch_optional(&mut **tx)
                        .await?
                        .ok_or(ApiError::Validation("import_unknown_request"))?;
                let body = orders::RecordOrderBody {
                    po_number: s.po_number.clone(),
                    notes: format!("Imported from {}", batch.file_name),
                };
                let (po_number, notes) = orders::clean_order(&body)?;
                let id =
                    orders::record_order_tx(tx, p, org_id, side, request_id, &po_number, &notes)
                        .await?;
                ("purchase_order", id, None)
            }
            ImportKind::Receipt => {
                let order_id = order_by_po(tx, &s.po_number)
                    .await?
                    .ok_or(ApiError::Validation("import_unknown_po"))?;
                let known = known_lines(tx, order_id).await?;
                let k: Vec<KnownLine> = known.iter().map(|k| k.1.clone()).collect();
                let mut lines = Vec::with_capacity(s.lines.len());
                for l in &s.lines {
                    let i = conn::match_line(&k, &record_line(l))
                        .ok_or(ApiError::Validation("import_unknown_item"))?;
                    lines.push(DeliveryLineInput {
                        order_item_id: known[i].0,
                        quantity: l.quantity.ok_or(ApiError::Validation("lines"))?,
                    });
                }
                let date = s.date.ok_or(ApiError::Validation("received_at"))?;
                // Noon on that day in the organisation's time zone (now, if that is later).
                let noon: DateTime<Utc> = sqlx::query_scalar(
                    "SELECT ($1::date + time '12:00') AT TIME ZONE timezone
                     FROM organisations WHERE id = $2",
                )
                .bind(date)
                .bind(org_id)
                .fetch_one(&mut **tx)
                .await?;
                let body = DeliveryInput {
                    received_at: Some(noon.min(Utc::now())),
                    note: s.note.clone(),
                    lines,
                };
                let (note, received_at) = orders::clean_delivery(&body)?;
                let id = orders::record_delivery_tx(
                    tx,
                    p,
                    org_id,
                    side,
                    view,
                    order_id,
                    &body,
                    &note,
                    received_at,
                )
                .await?;
                ("delivery", id, None)
            }
            ImportKind::Invoice => {
                let order_id = order_by_po(tx, &s.po_number)
                    .await?
                    .ok_or(ApiError::Validation("import_unknown_po"))?;
                let known = known_lines(tx, order_id).await?;
                let k: Vec<KnownLine> = known.iter().map(|k| k.1.clone()).collect();
                let mut lines = Vec::with_capacity(s.lines.len());
                for l in &s.lines {
                    // A line not on the order is recorded as billed; the match reports it.
                    let item = conn::match_line(&k, &record_line(l)).map(|i| known[i].0);
                    let description = if l.description.is_empty() {
                        l.mpn.clone()
                    } else {
                        l.description.clone()
                    };
                    if item.is_none() && description.is_empty() {
                        return Err(ApiError::Validation("import_unknown_item"));
                    }
                    lines.push(InvoiceLineInput {
                        order_item_id: item,
                        description,
                        quantity: l.quantity.ok_or(ApiError::Validation("lines"))?,
                        unit_price_minor: l
                            .unit_price_minor
                            .ok_or(ApiError::Validation("lines"))?,
                        tax_rate_bps: None,
                    });
                }
                let body = InvoiceInput {
                    supplier_invoice_number: s.invoice_number.clone(),
                    invoice_date: s.date.ok_or(ApiError::Validation("invoice_date"))?,
                    due_date: s.due_date,
                    currency: s.currency.clone(),
                    shipping_minor: s.shipping_minor.unwrap_or(0),
                    // Staged from the profile (rate from the file or the profile's default;
                    // a row without either never reaches this point).
                    tax_rate_bps: s.tax_rate_bps.ok_or(ApiError::Validation("tax_rate"))?,
                    tax_rounding: s.tax_rounding.clone().unwrap_or_else(|| "floor".into()),
                    tax_minor: s.tax_minor.ok_or(ApiError::Validation("tax_minor"))?,
                    lines,
                };
                let clean = orders::clean_invoice(&body)?;
                let r =
                    orders::record_invoice_tx(tx, p, org_id, side, order_id, &body, &clean).await?;
                ("invoice", r.id, Some(r.match_status))
            }
        };
        audit
            .record(
                tx,
                "import.record_applied",
                "import_batch",
                batch_id,
                json!({ "seq": seq, "rows": s.rows, "po_number": s.po_number,
                        "entity_type": entity_type, "entity_id": entity_id,
                        "match_status": match_status }),
            )
            .await?;
        applied.push(AppliedRecord {
            seq,
            po_number: s.po_number,
            entity_type: entity_type.into(),
            entity_id,
            match_status,
        });
    }
    sqlx::query(
        "UPDATE import_batches SET status = 'applied', decided_by = $2, decided_at = now()
         WHERE id = $1",
    )
    .bind(batch_id)
    .bind(p.user_id)
    .execute(&mut **tx)
    .await?;
    audit
        .record(
            tx,
            "import.applied",
            "import_batch",
            batch_id,
            json!({ "kind": batch.kind, "file_name": batch.file_name,
                    "records": applied.len() }),
        )
        .await?;
    Ok(applied)
}

// ---------------------------------------------------------------------------------------
// Exports

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AgentResultsQuery {
    pub agent: Option<String>,
    /// `open` | `approved` | `edited` | `rejected` | `resolved_elsewhere` (default: all)
    pub status: Option<String>,
}

#[derive(sqlx::FromRow)]
struct ResultRow {
    created_at: DateTime<Utc>,
    agent: String,
    kind: String,
    severity: String,
    status: String,
    title_key: String,
    subject_type: Option<String>,
    subject_id: Option<Uuid>,
    params: Value,
    proposed_action: Option<Value>,
    decided_action: Option<Value>,
    decided_by: Option<Uuid>,
    decided_at: Option<DateTime<Utc>>,
    decision_note: String,
}

#[utoipa::path(
    operation_id = "integrations_export_agent_results",
    get,
    path = "/api/v1/orgs/{org_id}/integrations/exports/agent-results.csv",
    tag = "integrations",
    params(("org_id" = Uuid, Path), AgentResultsQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Agent exceptions with their decisions as CSV (text/csv, UTF-8 with BOM; formula-like cells quoted)"),
        (status = 400, description = "Invalid status", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn export_agent_results(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiQuery(q): ApiQuery<AgentResultsQuery>,
) -> ApiResult<impl IntoResponse> {
    p.require(&state, org_id, Permission::ViewAgents).await?;
    if let Some(s) = &q.status
        && procure_domain::agents::ExceptionStatus::from_code(s).is_none()
    {
        return Err(ApiError::BadRequest("status"));
    }
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let rows = sqlx::query_as::<_, ResultRow>(
        "SELECT created_at, agent, kind, severity, status, title_key, subject_type, subject_id,
                params, proposed_action, decided_action, decided_by, decided_at, decision_note
         FROM agent_exceptions
         WHERE org_id = $1 AND ($2::text IS NULL OR agent = $2)
           AND ($3::text IS NULL OR status = $3)
         ORDER BY created_at, id LIMIT $4",
    )
    .bind(org_id)
    .bind(&q.agent)
    .bind(&q.status)
    .bind(MAX_EXPORT_ROWS)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let opt = |v: &Option<Value>| v.as_ref().map(Value::to_string).unwrap_or_default();
    let header: Vec<String> = [
        "created_at",
        "agent",
        "kind",
        "severity",
        "status",
        "title_key",
        "subject_type",
        "subject_id",
        "params",
        "proposed_action",
        "decided_action",
        "decided_by",
        "decided_at",
        "decision_note",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                r.created_at.to_rfc3339(),
                r.agent.clone(),
                r.kind.clone(),
                r.severity.clone(),
                r.status.clone(),
                r.title_key.clone(),
                r.subject_type.clone().unwrap_or_default(),
                r.subject_id.map(|u| u.to_string()).unwrap_or_default(),
                r.params.to_string(),
                opt(&r.proposed_action),
                opt(&r.decided_action),
                r.decided_by.map(|u| u.to_string()).unwrap_or_default(),
                r.decided_at.map(|d| d.to_rfc3339()).unwrap_or_default(),
                r.decision_note.clone(),
            ]
        })
        .collect();
    let export = Export {
        file_name: "agent-results.csv".into(),
        header,
        rows,
    };
    let body = String::from_utf8(
        FileConnector
            .push(&export)
            .await
            .map_err(|e| ApiError::internal(std::io::Error::other(e.code())))?,
    )
    .map_err(ApiError::internal)?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"agent-results.csv\""),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok((headers, body))
}
