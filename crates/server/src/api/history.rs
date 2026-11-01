//! A customer's decision material: past purchases imported from their own spreadsheets / ERP
//! exports, the price reference shown next to every request line, and their written buying
//! criteria. Imports are previewed first (nothing stored), then committed as one audited
//! batch that can be withdrawn as a whole.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use base64::Engine;
use chrono::{DateTime, NaiveDate, Utc};
use procure_domain::history::{self, Field, HistoryRow, PricePoint};
use procure_domain::money::Currency;
use procure_domain::rbac::{self, Access, Permission};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::api::common::{self, Audit};
use crate::db;
use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ApiResult, ErrorBody};
use crate::intake::{self, IntakeError};
use crate::principal::Principal;
use crate::storage::MAX_ATTACHMENT_BYTES;

/// Rows of a preview returned to the screen, and row errors listed by number.
const PREVIEW_ROWS: usize = 20;
const LISTED_ERRORS: usize = 50;
/// Past purchases considered per item for the price reference.
const REFERENCE_DEPTH: i64 = 200;
const RECENT_PER_ITEM: usize = 5;

/// Importing and maintaining the records: buyers in the customer, or the assigned service team
/// (who onboard the customer's data for them).
async fn require_manage(state: &AppState, p: &Principal, org_id: Uuid) -> ApiResult<Access> {
    p.require(state, org_id, Permission::ManagePurchaseHistory)
        .await
}

/// Reading: anyone who may manage the records or see all of the organisation's requests.
async fn require_read(state: &AppState, p: &Principal, org_id: Uuid) -> ApiResult<Access> {
    let access = p.access(state, org_id).await?;
    if access == Access::None {
        return Err(ApiError::NotFound);
    }
    if rbac::allows(access, Permission::ManagePurchaseHistory)
        || rbac::allows(access, Permission::ViewAllRequests)
    {
        Ok(access)
    } else {
        Err(ApiError::Forbidden)
    }
}

fn intake_error(e: IntakeError) -> ApiError {
    match e {
        IntakeError::Failed => ApiError::Conflict("intake_failed"),
        other => ApiError::Validation(other.code()),
    }
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ImportBody {
    /// `.xlsx`, `.csv` or `.tsv` (UTF-8 or Shift_JIS).
    pub file_name: String,
    /// File content, standard base64.
    pub data_base64: String,
    /// Header row (0-based); detected when omitted.
    pub header_row: Option<usize>,
    /// Field code per column (`date`, `supplier`, `description`, `manufacturer`, `mpn`,
    /// `quantity`, `unit`, `unit_price`, `line_total`, `currency`, `lead_time_days`, `notes`;
    /// null = ignore). Detected from the header when omitted.
    pub mapping: Option<Vec<Option<String>>>,
    /// Currency for rows without one; the organisation's usual currency, default `JPY`.
    pub currency: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct RowErrorView {
    /// 1-based row number in the file, as the spreadsheet shows it.
    pub row: usize,
    pub code: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ParsedRowView {
    pub row: usize,
    pub purchased_on: NaiveDate,
    pub supplier_name: String,
    pub description: String,
    pub manufacturer: String,
    pub mpn: String,
    /// Thousandths of a unit.
    pub quantity_milli: i64,
    pub unit: String,
    /// Thousandths of a minor currency unit.
    pub unit_price_milli: i64,
    pub price_derived: bool,
    pub currency: String,
    pub lead_time_days: Option<i32>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct PreviewView {
    pub header_row: usize,
    pub headers: Vec<String>,
    pub mapping: Vec<Option<String>>,
    /// Non-blank data rows in the file.
    pub data_rows: usize,
    pub valid_rows: usize,
    /// Count per reason code.
    pub error_counts: BTreeMap<String, usize>,
    /// The first rows that could not be read.
    pub errors: Vec<RowErrorView>,
    /// The first rows as they would be stored.
    pub sample: Vec<ParsedRowView>,
}

struct Parsed {
    preview: PreviewView,
    rows: Vec<(usize, HistoryRow)>,
}

fn day(d: history::Day) -> ApiResult<NaiveDate> {
    NaiveDate::from_ymd_opt(d.year, d.month, d.day)
        .ok_or_else(|| ApiError::internal(std::io::Error::other("invalid calendar day")))
}

async fn parse_file(body: &ImportBody, state: &AppState) -> ApiResult<Parsed> {
    let file_name = common::required(&body.file_name, 255, false, "file_name")?;
    let currency = match body.currency.as_deref() {
        None | Some("") => Currency::Jpy,
        Some(c) => common::currency(c)?,
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body.data_base64.as_bytes())
        .map_err(|_| ApiError::BadRequest("invalid_base64"))?;
    if bytes.is_empty() || bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(ApiError::Validation("file_size"));
    }
    let table = {
        let _permit = state
            .upload_permits
            .acquire()
            .await
            .map_err(ApiError::internal)?;
        intake::table_rows(&file_name, &bytes)
            .await
            .map_err(intake_error)?
    };
    let header_row = match body.header_row {
        Some(h) if h < table.len().min(50) => h,
        Some(_) => return Err(ApiError::Validation("header_row")),
        None => history::find_header(&table).ok_or(ApiError::Validation("header_not_found"))?,
    };
    let headers = table[header_row].clone();
    let mapping: Vec<Option<Field>> = match &body.mapping {
        None => history::detect_columns(&headers),
        Some(m) => {
            if m.len() > intake::MAX_TABLE_COLUMNS {
                return Err(ApiError::Validation("mapping"));
            }
            let fields = m
                .iter()
                .map(|c| match c.as_deref() {
                    None | Some("") => Ok(None),
                    Some(code) => Field::from_code(code)
                        .map(Some)
                        .ok_or(ApiError::Validation("mapping")),
                })
                .collect::<ApiResult<Vec<_>>>()?;
            let mut seen: Vec<Field> = fields.iter().flatten().copied().collect();
            seen.sort_by_key(|f| f.code());
            if seen.windows(2).any(|w| w[0] == w[1]) {
                return Err(ApiError::Validation("mapping_duplicate"));
            }
            fields
        }
    };
    let has = |f: Field| mapping.contains(&Some(f));
    if !has(Field::Date)
        || !(has(Field::Description) || has(Field::Mpn))
        || !(has(Field::UnitPrice) || has(Field::LineTotal))
    {
        return Err(ApiError::Validation("mapping_incomplete"));
    }

    let mut data_rows = 0;
    let mut rows = Vec::new();
    let mut error_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut errors = Vec::new();
    for (i, cells) in table.iter().enumerate().skip(header_row + 1) {
        if history::is_blank(cells) {
            continue;
        }
        data_rows += 1;
        match history::parse_row(cells, &mapping, currency) {
            Ok(r) => rows.push((i + 1, r)),
            Err(e) => {
                *error_counts.entry(e.code().to_string()).or_default() += 1;
                if errors.len() < LISTED_ERRORS {
                    errors.push(RowErrorView {
                        row: i + 1,
                        code: e.code().to_string(),
                    });
                }
            }
        }
    }
    if data_rows > intake::MAX_TABLE_ROWS {
        return Err(ApiError::Validation("too_many_rows"));
    }
    let sample = rows
        .iter()
        .take(PREVIEW_ROWS)
        .map(|(n, r)| {
            Ok(ParsedRowView {
                row: *n,
                purchased_on: day(r.purchased_on)?,
                supplier_name: r.supplier.clone(),
                description: r.description.clone(),
                manufacturer: r.manufacturer.clone(),
                mpn: r.mpn.clone(),
                quantity_milli: r.quantity_milli,
                unit: r.unit.clone(),
                unit_price_milli: r.unit_price_milli,
                price_derived: r.price_derived,
                currency: r.currency.code().to_string(),
                lead_time_days: r.lead_time_days,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Parsed {
        preview: PreviewView {
            header_row,
            headers,
            mapping: mapping
                .iter()
                .map(|m| m.map(|f| f.code().to_string()))
                .collect(),
            data_rows,
            valid_rows: rows.len(),
            error_counts,
            errors,
            sample,
        },
        rows,
    })
}

#[utoipa::path(
    operation_id = "purchase_history_preview",
    post,
    path = "/api/v1/orgs/{org_id}/purchase-history/preview",
    tag = "purchase_history",
    params(("org_id" = Uuid, Path)),
    request_body = ImportBody,
    security(("bearer" = [])),
    responses(
        (status = 200, description = "How the file would be read; nothing is stored", body = PreviewView),
        (status = 422, description = "Unsupported file, no header found, incomplete or invalid column mapping, too many rows", body = ErrorBody),
        (status = 413, description = "Larger than 10 MB"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn preview(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<ImportBody>,
) -> ApiResult<Json<PreviewView>> {
    require_manage(&state, &p, org_id).await?;
    Ok(Json(parse_file(&body, &state).await?.preview))
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct ImportView {
    pub id: Uuid,
    pub file_name: String,
    pub rows_imported: i32,
    pub rows_skipped: i32,
    pub imported_by_name: Option<String>,
    pub imported_at: DateTime<Utc>,
    pub withdrawn_at: Option<DateTime<Utc>>,
}

const IMPORT_COLUMNS: &str = "i.id, i.file_name, i.rows_imported, i.rows_skipped,
    u.display_name AS imported_by_name, i.imported_at, i.withdrawn_at";

/// Rows stored per statement.
const INSERT_CHUNK: usize = 2_000;

#[utoipa::path(
    operation_id = "purchase_history_import",
    post,
    path = "/api/v1/orgs/{org_id}/purchase-history/imports",
    tag = "purchase_history",
    params(("org_id" = Uuid, Path)),
    request_body = ImportBody,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Readable rows stored as one import; unreadable rows skipped and counted", body = ImportView),
        (status = 422, description = "As for preview, or no readable rows", body = ErrorBody),
        (status = 413, description = "Larger than 10 MB"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn import(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<ImportBody>,
) -> ApiResult<(StatusCode, Json<ImportView>)> {
    require_manage(&state, &p, org_id).await?;
    let parsed = parse_file(&body, &state).await?;
    if parsed.rows.is_empty() {
        return Err(ApiError::Validation("nothing_readable"));
    }
    let id = Uuid::now_v7();
    let skipped = parsed.preview.data_rows - parsed.rows.len();
    let count = |n: usize| i32::try_from(n).map_err(ApiError::internal);
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    sqlx::query(
        "INSERT INTO history_imports (id, org_id, file_name, mapping, rows_imported, rows_skipped,
                                      imported_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(org_id)
    .bind(body.file_name.trim())
    .bind(json!(parsed.preview.mapping))
    .bind(count(parsed.rows.len())?)
    .bind(count(skipped)?)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    for chunk in parsed.rows.chunks(INSERT_CHUNK) {
        let mut ids = Vec::with_capacity(chunk.len());
        let mut source_rows = Vec::with_capacity(chunk.len());
        let mut dates = Vec::with_capacity(chunk.len());
        let mut suppliers = Vec::with_capacity(chunk.len());
        let mut descriptions = Vec::with_capacity(chunk.len());
        let mut makers = Vec::with_capacity(chunk.len());
        let mut mpns = Vec::with_capacity(chunk.len());
        let mut keys = Vec::with_capacity(chunk.len());
        let mut quantities = Vec::with_capacity(chunk.len());
        let mut units = Vec::with_capacity(chunk.len());
        let mut prices = Vec::with_capacity(chunk.len());
        let mut derived = Vec::with_capacity(chunk.len());
        let mut currencies = Vec::with_capacity(chunk.len());
        let mut lead_times: Vec<Option<i32>> = Vec::with_capacity(chunk.len());
        let mut notes = Vec::with_capacity(chunk.len());
        for (n, r) in chunk {
            // A row always names an item, so it always has a key.
            let Some(key) = history::item_key(&r.mpn, &r.description) else {
                continue;
            };
            ids.push(Uuid::now_v7());
            source_rows.push(count(*n)?);
            dates.push(day(r.purchased_on)?);
            suppliers.push(r.supplier.clone());
            descriptions.push(r.description.clone());
            makers.push(r.manufacturer.clone());
            mpns.push(r.mpn.clone());
            keys.push(key);
            quantities.push(r.quantity_milli);
            units.push(r.unit.clone());
            prices.push(r.unit_price_milli);
            derived.push(r.price_derived);
            currencies.push(r.currency.code().to_string());
            lead_times.push(r.lead_time_days);
            notes.push(r.notes.clone());
        }
        sqlx::query(
            "INSERT INTO purchase_history (id, org_id, import_id, source_row, purchased_on,
                 supplier_name, description, manufacturer, mpn, item_key, quantity_milli, unit,
                 unit_price_milli, price_derived, currency, lead_time_days, notes)
             SELECT u.id, $1, $2, u.source_row, u.purchased_on, u.supplier_name, u.description,
                    u.manufacturer, u.mpn, u.item_key, u.quantity_milli, u.unit,
                    u.unit_price_milli, u.price_derived, u.currency, u.lead_time_days, u.notes
             FROM UNNEST($3::uuid[], $4::int[], $5::date[], $6::text[], $7::text[], $8::text[],
                         $9::text[], $10::text[], $11::bigint[], $12::text[], $13::bigint[],
                         $14::bool[], $15::text[], $16::int[], $17::text[])
                  AS u(id, source_row, purchased_on, supplier_name, description, manufacturer,
                       mpn, item_key, quantity_milli, unit, unit_price_milli, price_derived,
                       currency, lead_time_days, notes)",
        )
        .bind(org_id)
        .bind(id)
        .bind(&ids)
        .bind(&source_rows)
        .bind(&dates)
        .bind(&suppliers)
        .bind(&descriptions)
        .bind(&makers)
        .bind(&mpns)
        .bind(&keys)
        .bind(&quantities)
        .bind(&units)
        .bind(&prices)
        .bind(&derived)
        .bind(&currencies)
        .bind(&lead_times)
        .bind(&notes)
        .execute(&mut *tx)
        .await?;
    }
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "purchase_history.imported",
            "history_import",
            id,
            json!({
                "file_name": body.file_name.trim(),
                "rows_imported": parsed.rows.len(),
                "rows_skipped": skipped,
            }),
        )
        .await?;
    let view = sqlx::query_as::<_, ImportView>(&format!(
        "SELECT {IMPORT_COLUMNS} FROM history_imports i LEFT JOIN users u ON u.id = i.imported_by
         WHERE i.id = $1"
    ))
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(view)))
}

#[utoipa::path(
    operation_id = "purchase_history_imports",
    get,
    path = "/api/v1/orgs/{org_id}/purchase-history/imports",
    tag = "purchase_history",
    params(("org_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Imports, newest first (withdrawn ones included)", body = [ImportView]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list_imports(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
) -> ApiResult<Json<Vec<ImportView>>> {
    require_read(&state, &p, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let rows = sqlx::query_as::<_, ImportView>(&format!(
        "SELECT {IMPORT_COLUMNS} FROM history_imports i LEFT JOIN users u ON u.id = i.imported_by
         ORDER BY i.imported_at DESC, i.id DESC LIMIT 200"
    ))
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[utoipa::path(
    operation_id = "purchase_history_withdraw",
    post,
    path = "/api/v1/orgs/{org_id}/purchase-history/imports/{id}/withdraw",
    tag = "purchase_history",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "The import's rows are removed; the import stays listed as withdrawn", body = ImportView),
        (status = 409, description = "Already withdrawn", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown import", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn withdraw(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<ImportView>> {
    require_manage(&state, &p, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let withdrawn: Option<Option<DateTime<Utc>>> =
        sqlx::query_scalar("SELECT withdrawn_at FROM history_imports WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    match withdrawn {
        None => return Err(ApiError::NotFound),
        Some(Some(_)) => return Err(ApiError::Conflict("already_withdrawn")),
        Some(None) => {}
    }
    let removed = sqlx::query("DELETE FROM purchase_history WHERE import_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    sqlx::query("UPDATE history_imports SET withdrawn_at = now(), withdrawn_by = $2 WHERE id = $1")
        .bind(id)
        .bind(p.user_id)
        .execute(&mut *tx)
        .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "purchase_history.withdrawn",
            "history_import",
            id,
            json!({ "rows_removed": removed }),
        )
        .await?;
    let view = sqlx::query_as::<_, ImportView>(&format!(
        "SELECT {IMPORT_COLUMNS} FROM history_imports i LEFT JOIN users u ON u.id = i.imported_by
         WHERE i.id = $1"
    ))
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(view))
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct HistoryRowView {
    pub id: Uuid,
    pub import_id: Uuid,
    pub purchased_on: NaiveDate,
    pub supplier_name: String,
    pub description: String,
    pub manufacturer: String,
    pub mpn: String,
    /// Thousandths of a unit.
    pub quantity_milli: i64,
    pub unit: String,
    /// Thousandths of a minor currency unit.
    pub unit_price_milli: i64,
    pub price_derived: bool,
    pub currency: String,
    pub lead_time_days: Option<i32>,
    pub notes: String,
}

const ROW_COLUMNS: &str = "id, import_id, purchased_on, supplier_name, description, manufacturer,
    mpn, quantity_milli, unit, unit_price_milli, price_derived, currency, lead_time_days, notes";

#[derive(Deserialize, utoipa::IntoParams)]
pub struct SearchQuery {
    /// Words found in the description, part number, maker or supplier.
    pub q: Option<String>,
    pub limit: Option<i64>,
}

#[utoipa::path(
    operation_id = "purchase_history_search",
    get,
    path = "/api/v1/orgs/{org_id}/purchase-history",
    tag = "purchase_history",
    params(("org_id" = Uuid, Path), SearchQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Past purchases, newest first", body = [HistoryRowView]),
        (status = 422, description = "Search text too long", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn search(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiQuery(q): ApiQuery<SearchQuery>,
) -> ApiResult<Json<Vec<HistoryRowView>>> {
    require_read(&state, &p, org_id).await?;
    let text = common::text(q.q.as_deref().unwrap_or(""), 200, false, "q")?;
    // Every word must appear somewhere in the row (LIKE wildcards in the input are literal).
    let words: Vec<String> = history::half_width(&text)
        .split_whitespace()
        .take(8)
        .map(|w| {
            let escaped = w
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            format!("%{escaped}%")
        })
        .collect();
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let rows = sqlx::query_as::<_, HistoryRowView>(&format!(
        "SELECT {ROW_COLUMNS} FROM purchase_history h
         WHERE NOT EXISTS (
             SELECT 1 FROM unnest($1::text[]) w
             WHERE (h.description || ' ' || h.mpn || ' ' || h.manufacturer || ' ' || h.supplier_name)
                   NOT ILIKE w)
         ORDER BY purchased_on DESC, id DESC LIMIT $2"
    ))
    .bind(&words)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct PricePointView {
    pub purchased_on: NaiveDate,
    pub supplier_name: String,
    pub unit_price_milli: i64,
    pub currency: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct PriceSummaryView {
    pub purchases: usize,
    pub last: PricePointView,
    pub min_price_milli: i64,
    pub max_price_milli: i64,
    pub cheapest: PricePointView,
    pub suppliers: usize,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ItemReferenceView {
    pub item_id: Uuid,
    pub line_no: i32,
    /// `mpn` or `description`: what the past purchases were matched on; null = nothing to match.
    pub matched_on: Option<String>,
    pub summary: Option<PriceSummaryView>,
    /// The most recent matching purchases.
    pub recent: Vec<HistoryRowView>,
}

fn point(p: &PricePoint) -> ApiResult<PricePointView> {
    Ok(PricePointView {
        purchased_on: day(p.purchased_on)?,
        supplier_name: p.supplier.clone(),
        unit_price_milli: p.unit_price_milli,
        currency: p.currency.code().to_string(),
    })
}

#[derive(sqlx::FromRow)]
struct ItemRow {
    id: Uuid,
    line_no: i32,
    description: String,
    mpn: String,
    verified_mpn: Option<String>,
}

#[derive(sqlx::FromRow)]
struct KeyedRow {
    item_key: String,
    #[sqlx(flatten)]
    row: HistoryRowView,
}

#[utoipa::path(
    operation_id = "request_price_reference",
    get,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/price-reference",
    tag = "purchase_history",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Per request line: what was paid before, to whom, and when (matched on the verified or given part number, else the description)", body = [ItemReferenceView]),
        (status = 403, description = "Not permitted (requesters see their requests but not past prices)", body = ErrorBody),
        (status = 404, description = "Unknown request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn price_reference(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<Vec<ItemReferenceView>>> {
    require_read(&state, &p, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM procurement_requests WHERE id = $1)")
            .bind(request_id)
            .fetch_one(&mut *tx)
            .await?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    let items = sqlx::query_as::<_, ItemRow>(
        "SELECT id, line_no, description, mpn, verified_mpn FROM request_items
         WHERE request_id = $1 ORDER BY line_no",
    )
    .bind(request_id)
    .fetch_all(&mut *tx)
    .await?;
    // A human-verified part number wins over the one typed in the request.
    let keys: Vec<Option<String>> = items
        .iter()
        .map(|i| history::item_key(i.verified_mpn.as_deref().unwrap_or(&i.mpn), &i.description))
        .collect();
    let wanted: Vec<String> = keys.iter().flatten().cloned().collect();
    let found = sqlx::query_as::<_, KeyedRow>(&format!(
        "SELECT item_key, {ROW_COLUMNS} FROM (
             SELECT *, row_number() OVER (PARTITION BY item_key
                                          ORDER BY purchased_on DESC, id DESC) AS rn
             FROM purchase_history WHERE item_key = ANY($1)) h
         WHERE rn <= $2 ORDER BY item_key, purchased_on DESC, id DESC"
    ))
    .bind(&wanted)
    .bind(REFERENCE_DEPTH)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let mut by_key: BTreeMap<String, Vec<HistoryRowView>> = BTreeMap::new();
    for k in found {
        by_key.entry(k.item_key).or_default().push(k.row);
    }
    let mut out = Vec::with_capacity(items.len());
    for (item, key) in items.into_iter().zip(keys) {
        let rows = key
            .as_ref()
            .and_then(|k| by_key.remove(k))
            .unwrap_or_default();
        let points = rows
            .iter()
            .map(|r| {
                Ok(PricePoint {
                    purchased_on: history::Day::new(
                        chrono::Datelike::year(&r.purchased_on),
                        chrono::Datelike::month(&r.purchased_on),
                        chrono::Datelike::day(&r.purchased_on),
                    )
                    .ok_or_else(|| ApiError::internal(std::io::Error::other("stored date")))?,
                    supplier: r.supplier_name.clone(),
                    unit_price_milli: r.unit_price_milli,
                    currency: Currency::from_code(&r.currency)
                        .ok_or_else(|| ApiError::internal(std::io::Error::other("currency")))?,
                })
            })
            .collect::<ApiResult<Vec<_>>>()?;
        let summary = match history::summarise(&points) {
            None => None,
            Some(s) => Some(PriceSummaryView {
                purchases: s.purchases,
                last: point(&s.last)?,
                min_price_milli: s.min_price_milli,
                max_price_milli: s.max_price_milli,
                cheapest: point(&s.cheapest)?,
                suppliers: s.suppliers,
            }),
        };
        out.push(ItemReferenceView {
            item_id: item.id,
            line_no: item.line_no,
            matched_on: key.map(|k| {
                if k.starts_with("m:") {
                    "mpn"
                } else {
                    "description"
                }
                .to_string()
            }),
            summary,
            recent: rows.into_iter().take(RECENT_PER_ITEM).collect(),
        });
    }
    Ok(Json(out))
}

pub const PRIORITIES: [&str; 5] = [
    "price",
    "lead_time",
    "quality",
    "existing_supplier",
    "domestic",
];

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct CriteriaInput {
    /// Most important first; each of `price`, `lead_time`, `quality`, `existing_supplier`,
    /// `domestic` at most once.
    #[serde(default)]
    pub priorities: Vec<String>,
    #[serde(default)]
    pub preferred_suppliers: Vec<String>,
    #[serde(default)]
    pub excluded_suppliers: Vec<String>,
    /// E.g. ISO 9001, RoHS, JIS.
    #[serde(default)]
    pub required_certifications: Vec<String>,
    pub max_lead_time_days: Option<i32>,
    /// Flag quotes this much above the last price paid (basis points, 1000 = 10 %).
    pub price_alert_bps: Option<i32>,
    #[serde(default)]
    pub notes: String,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct CriteriaView {
    pub priorities: Vec<String>,
    pub preferred_suppliers: Vec<String>,
    pub excluded_suppliers: Vec<String>,
    pub required_certifications: Vec<String>,
    pub max_lead_time_days: Option<i32>,
    pub price_alert_bps: Option<i32>,
    pub notes: String,
    pub updated_by_name: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

fn names(list: &[String], max: usize, field: &'static str) -> ApiResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for raw in list {
        let n = common::text(raw, 200, false, field)?;
        if !n.is_empty() && !out.contains(&n) {
            out.push(n);
        }
    }
    if out.len() > max {
        return Err(ApiError::Validation(field));
    }
    Ok(out)
}

fn clean_criteria(c: &CriteriaInput) -> ApiResult<CriteriaInput> {
    let mut priorities: Vec<String> = Vec::new();
    for pr in &c.priorities {
        if !PRIORITIES.contains(&pr.as_str()) || priorities.contains(pr) {
            return Err(ApiError::Validation("priorities"));
        }
        priorities.push(pr.clone());
    }
    let preferred = names(&c.preferred_suppliers, 50, "preferred_suppliers")?;
    let excluded = names(&c.excluded_suppliers, 50, "excluded_suppliers")?;
    if preferred.iter().any(|s| excluded.contains(s)) {
        return Err(ApiError::Validation("supplier_both_lists"));
    }
    if c.max_lead_time_days
        .is_some_and(|d| !(1..=3650).contains(&d))
    {
        return Err(ApiError::Validation("max_lead_time_days"));
    }
    if c.price_alert_bps
        .is_some_and(|b| !(0..=100_000).contains(&b))
    {
        return Err(ApiError::Validation("price_alert_bps"));
    }
    Ok(CriteriaInput {
        priorities,
        preferred_suppliers: preferred,
        excluded_suppliers: excluded,
        required_certifications: names(&c.required_certifications, 20, "required_certifications")?,
        max_lead_time_days: c.max_lead_time_days,
        price_alert_bps: c.price_alert_bps,
        notes: common::text(&c.notes, 4000, true, "notes")?,
    })
}

const CRITERIA_COLUMNS: &str = "c.priorities, c.preferred_suppliers, c.excluded_suppliers,
    c.required_certifications, c.max_lead_time_days, c.price_alert_bps, c.notes,
    u.display_name AS updated_by_name, c.updated_at";

async fn load_criteria(tx: &mut db::Tx) -> ApiResult<CriteriaView> {
    let row = sqlx::query_as::<_, CriteriaView>(&format!(
        "SELECT {CRITERIA_COLUMNS} FROM buying_criteria c
         LEFT JOIN users u ON u.id = c.updated_by"
    ))
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.unwrap_or(CriteriaView {
        priorities: Vec::new(),
        preferred_suppliers: Vec::new(),
        excluded_suppliers: Vec::new(),
        required_certifications: Vec::new(),
        max_lead_time_days: None,
        price_alert_bps: None,
        notes: String::new(),
        updated_by_name: None,
        updated_at: None,
    }))
}

#[utoipa::path(
    operation_id = "buying_criteria_get",
    get,
    path = "/api/v1/orgs/{org_id}/buying-criteria",
    tag = "purchase_history",
    params(("org_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "The organisation's buying criteria (empty until written)", body = CriteriaView),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn get_criteria(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
) -> ApiResult<Json<CriteriaView>> {
    require_read(&state, &p, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let view = load_criteria(&mut tx).await?;
    tx.commit().await?;
    Ok(Json(view))
}

#[utoipa::path(
    operation_id = "buying_criteria_put",
    put,
    path = "/api/v1/orgs/{org_id}/buying-criteria",
    tag = "purchase_history",
    params(("org_id" = Uuid, Path)),
    request_body = CriteriaInput,
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Saved (audited with the previous version)", body = CriteriaView),
        (status = 422, description = "Invalid priority, list or limit", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn put_criteria(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CriteriaInput>,
) -> ApiResult<Json<CriteriaView>> {
    require_manage(&state, &p, org_id).await?;
    let c = clean_criteria(&body)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let before = load_criteria(&mut tx).await?;
    sqlx::query(
        "INSERT INTO buying_criteria (org_id, priorities, preferred_suppliers, excluded_suppliers,
             required_certifications, max_lead_time_days, price_alert_bps, notes, updated_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         ON CONFLICT (org_id) DO UPDATE SET priorities = $2, preferred_suppliers = $3,
             excluded_suppliers = $4, required_certifications = $5, max_lead_time_days = $6,
             price_alert_bps = $7, notes = $8, updated_by = $9, updated_at = now()",
    )
    .bind(org_id)
    .bind(&c.priorities)
    .bind(&c.preferred_suppliers)
    .bind(&c.excluded_suppliers)
    .bind(&c.required_certifications)
    .bind(c.max_lead_time_days)
    .bind(c.price_alert_bps)
    .bind(&c.notes)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "buying_criteria.updated",
            "organisation",
            org_id,
            json!({
                "before": {
                    "priorities": before.priorities,
                    "preferred_suppliers": before.preferred_suppliers,
                    "excluded_suppliers": before.excluded_suppliers,
                    "required_certifications": before.required_certifications,
                    "max_lead_time_days": before.max_lead_time_days,
                    "price_alert_bps": before.price_alert_bps,
                    "notes": before.notes,
                },
                "after": c,
            }),
        )
        .await?;
    let view = load_criteria(&mut tx).await?;
    tx.commit().await?;
    Ok(Json(view))
}
