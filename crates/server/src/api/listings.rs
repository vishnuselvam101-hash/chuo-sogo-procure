//! Supplier product listings and the catalogue (Phase C).
//!
//! Suppliers (`manage_listings`) write listings with photos and documents and submit them;
//! Chuo-Sogo (`moderate_listings`, admin console) approves or rejects; approved listings form
//! the catalogue that customers (`view_catalogue`) browse. Any change by the supplier takes a
//! listing back to draft, so nothing unreviewed is ever shown. Photos are stored without
//! their metadata (see `media`).

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::IntoResponse;
use base64::Engine;
use chrono::{DateTime, Utc};
use procure_domain::listing::{
    self, Action, Actor, Completeness, ListingStatus, MAX_DOCUMENTS, MAX_PHOTOS, MAX_SPECS,
};
use procure_domain::rbac::{Access, Permission};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::api::admin::AdminPrincipal;
use crate::api::common::{self, Audit};
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ApiResult, ErrorBody};
use crate::media;
use crate::principal::Principal;
use crate::storage::{self, MAX_ATTACHMENT_BYTES};

#[derive(Deserialize, Serialize, Clone, utoipa::ToSchema)]
pub struct Spec {
    pub key: String,
    pub value: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ListingInput {
    #[serde(default)]
    pub manufacturer: String,
    #[serde(default)]
    pub model_number: String,
    /// Only for individual units (used / refurbished / one-off).
    pub serial_number: Option<String>,
    /// `new`, `used` or `refurbished`.
    pub condition: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub title_en: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub description_en: String,
    #[serde(default)]
    pub specs: Vec<Spec>,
    /// ISO 4217; default JPY.
    pub currency: Option<String>,
    /// Business price, tax excluded (minor units).
    pub business_price_minor: Option<i64>,
    /// Consumer price, tax included (minor units).
    pub consumer_price_minor: Option<i64>,
    /// 1000, 800 (reduced rate) or 0.
    pub tax_rate_bps: i32,
    /// Units in stock; empty = on request.
    pub stock: Option<i32>,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct ListingSummary {
    pub id: Uuid,
    pub org_id: Uuid,
    pub supplier_name: String,
    /// `draft`, `submitted`, `approved`, `rejected` or `retired`.
    pub status: String,
    pub title: String,
    pub manufacturer: String,
    pub model_number: String,
    pub condition: String,
    pub currency: String,
    pub business_price_minor: Option<i64>,
    pub consumer_price_minor: Option<i64>,
    pub stock: Option<i32>,
    /// First photo (shown as the cover).
    pub cover_media_id: Option<Uuid>,
    pub photo_count: i64,
    pub submitted_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct MediaView {
    pub id: Uuid,
    /// `photo` or `document`.
    pub kind: String,
    pub position: i32,
    pub file_name: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub width: Option<i32>,
    pub height: Option<i32>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ListingDetail {
    #[serde(flatten)]
    pub summary: ListingSummary,
    pub serial_number: Option<String>,
    pub title_en: String,
    pub description: String,
    pub description_en: String,
    pub specs: Vec<Spec>,
    pub tax_rate_bps: i32,
    pub media: Vec<MediaView>,
    /// Reviewer's note (reason for a rejection). Not shown in the catalogue.
    pub review_note: String,
    /// What is still missing before the listing can be submitted (stable codes).
    pub missing: Vec<String>,
    /// Moderation only: other active listings with the same manufacturer and model number.
    pub duplicates: Vec<ListingSummary>,
}

const SUMMARY_SELECT: &str = "SELECT l.id, l.org_id, coalesce(o.name, '') AS supplier_name, l.status,
        l.title, l.manufacturer, l.model_number, l.condition, l.currency,
        l.business_price_minor, l.consumer_price_minor, l.stock,
        (SELECT m.id FROM listing_media m WHERE m.listing_id = l.id AND m.kind = 'photo'
         ORDER BY m.position LIMIT 1) AS cover_media_id,
        (SELECT count(*) FROM listing_media m WHERE m.listing_id = l.id AND m.kind = 'photo') AS photo_count,
        l.submitted_at, l.updated_at
    FROM product_listings l LEFT JOIN organisations o ON o.id = l.org_id";

#[derive(sqlx::FromRow)]
struct DetailRow {
    serial_number: Option<String>,
    title_en: String,
    description: String,
    description_en: String,
    specs: serde_json::Value,
    tax_rate_bps: i32,
    review_note: String,
}

async fn load_detail(tx: &mut Tx, id: Uuid, moderation: bool) -> ApiResult<ListingDetail> {
    let summary = sqlx::query_as::<_, ListingSummary>(&format!("{SUMMARY_SELECT} WHERE l.id = $1"))
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    let d = sqlx::query_as::<_, DetailRow>(
        "SELECT serial_number, title_en, description, description_en, specs, tax_rate_bps, review_note
         FROM product_listings WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    let media = sqlx::query_as::<_, MediaView>(
        "SELECT id, kind, position, file_name, content_type, size_bytes, width, height
         FROM listing_media WHERE listing_id = $1 ORDER BY kind DESC, position",
    )
    .bind(id)
    .fetch_all(&mut **tx)
    .await?;
    let photos = media.iter().filter(|m| m.kind == "photo").count();
    let missing = listing::missing_for_submit(&Completeness {
        title: !summary.title.trim().is_empty(),
        manufacturer: !summary.manufacturer.trim().is_empty(),
        model_number: !summary.model_number.trim().is_empty(),
        description: !d.description.trim().is_empty(),
        photos,
        has_price: summary.business_price_minor.is_some() || summary.consumer_price_minor.is_some(),
    })
    .into_iter()
    .map(String::from)
    .collect();
    let duplicates = if moderation {
        sqlx::query_as::<_, ListingSummary>(&format!(
            "{SUMMARY_SELECT} WHERE l.id <> $1 AND l.status IN ('submitted', 'approved')
               AND l.model_key <> '' AND l.model_key = (SELECT model_key FROM product_listings WHERE id = $1)
               AND lower(btrim(l.manufacturer)) = lower(btrim($2))
             ORDER BY l.updated_at DESC LIMIT 20"
        ))
        .bind(id)
        .bind(&summary.manufacturer)
        .fetch_all(&mut **tx)
        .await?
    } else {
        Vec::new()
    };
    Ok(ListingDetail {
        summary,
        serial_number: d.serial_number,
        title_en: d.title_en,
        description: d.description,
        description_en: d.description_en,
        specs: serde_json::from_value(d.specs).map_err(ApiError::internal)?,
        tax_rate_bps: d.tax_rate_bps,
        media,
        review_note: d.review_note,
        missing,
        duplicates,
    })
}

// ---------------------------------------------------------------------------------------
// Supplier side.

/// A supplier member with `manage_listings` in `org_id`.
async fn supplier_tx(state: &AppState, p: &Principal, org_id: Uuid) -> ApiResult<Tx> {
    let access = p.access(state, org_id).await?;
    if !matches!(access, Access::Member(_)) {
        return Err(ApiError::NotFound);
    }
    common::require_perm(access, Permission::ManageListings)?;
    Ok(db::tenant_tx(&state.pool, p.user_id, org_id).await?)
}

struct Clean {
    manufacturer: String,
    model_number: String,
    serial_number: Option<String>,
    condition: String,
    title: String,
    title_en: String,
    description: String,
    description_en: String,
    specs: serde_json::Value,
    currency: String,
    business: Option<i64>,
    consumer: Option<i64>,
    tax_rate_bps: i32,
    stock: Option<i32>,
}

fn clean(input: &ListingInput) -> ApiResult<Clean> {
    if !["new", "used", "refurbished"].contains(&input.condition.as_str()) {
        return Err(ApiError::Validation("condition"));
    }
    if !matches!(input.tax_rate_bps, 0 | 800 | 1000) {
        return Err(ApiError::Validation("tax_rate_bps"));
    }
    if input.specs.len() > MAX_SPECS {
        return Err(ApiError::Validation("specs"));
    }
    let mut specs = Vec::with_capacity(input.specs.len());
    for s in &input.specs {
        specs.push(Spec {
            key: common::required(&s.key, 100, false, "specs")?,
            value: common::required(&s.value, 500, false, "specs")?,
        });
    }
    for price in [input.business_price_minor, input.consumer_price_minor]
        .into_iter()
        .flatten()
    {
        if price < 0 {
            return Err(ApiError::Validation("price"));
        }
    }
    if input
        .stock
        .is_some_and(|s| !(0..=1_000_000_000).contains(&s))
    {
        return Err(ApiError::Validation("stock"));
    }
    let serial = match input.serial_number.as_deref().map(str::trim) {
        Some("") | None => None,
        Some(s) => Some(common::required(s, 100, false, "serial_number")?),
    };
    if serial.is_some() && input.stock.is_some_and(|s| s > 1) {
        return Err(ApiError::Validation("stock"));
    }
    let currency = common::currency(input.currency.as_deref().unwrap_or("JPY"))?;
    Ok(Clean {
        manufacturer: common::text(&input.manufacturer, 200, false, "manufacturer")?,
        model_number: common::text(&input.model_number, 100, false, "model_number")?,
        serial_number: serial,
        condition: input.condition.clone(),
        title: common::text(&input.title, 200, false, "title")?,
        title_en: common::text(&input.title_en, 200, false, "title_en")?,
        description: common::text(&input.description, 5000, true, "description")?,
        description_en: common::text(&input.description_en, 5000, true, "description_en")?,
        specs: serde_json::to_value(&specs).map_err(ApiError::internal)?,
        currency: currency.code().to_string(),
        business: input.business_price_minor,
        consumer: input.consumer_price_minor,
        tax_rate_bps: input.tax_rate_bps,
        stock: input.stock,
    })
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct CreatedListing {
    pub id: Uuid,
}

#[utoipa::path(
    operation_id = "listing_list_own",
    get,
    path = "/api/v1/orgs/{org_id}/listings",
    tag = "listings",
    params(("org_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "The supplier's own listings, newest first", body = [ListingSummary]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list_own(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
) -> ApiResult<Json<Vec<ListingSummary>>> {
    let mut tx = supplier_tx(&state, &p, org_id).await?;
    let rows = sqlx::query_as::<_, ListingSummary>(&format!(
        "{SUMMARY_SELECT} WHERE l.org_id = $1 ORDER BY l.updated_at DESC LIMIT 500"
    ))
    .bind(org_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[utoipa::path(
    operation_id = "listing_create",
    post,
    path = "/api/v1/orgs/{org_id}/listings",
    tag = "listings",
    params(("org_id" = Uuid, Path)),
    request_body = ListingInput,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Draft listing created", body = CreatedListing),
        (status = 422, description = "Invalid input", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<ListingInput>,
) -> ApiResult<(StatusCode, Json<CreatedListing>)> {
    let c = clean(&body)?;
    let mut tx = supplier_tx(&state, &p, org_id).await?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO product_listings (id, org_id, manufacturer, model_number, model_key, serial_number,
             condition, title, title_en, description, description_en, specs, currency,
             business_price_minor, consumer_price_minor, tax_rate_bps, stock, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)",
    )
    .bind(id)
    .bind(org_id)
    .bind(&c.manufacturer)
    .bind(&c.model_number)
    .bind(listing::normalise_model(&c.model_number))
    .bind(&c.serial_number)
    .bind(&c.condition)
    .bind(&c.title)
    .bind(&c.title_en)
    .bind(&c.description)
    .bind(&c.description_en)
    .bind(&c.specs)
    .bind(&c.currency)
    .bind(c.business)
    .bind(c.consumer)
    .bind(c.tax_rate_bps)
    .bind(c.stock)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "listing.created",
            "listing",
            id,
            json!({ "title": c.title, "model_number": c.model_number }),
        )
        .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(CreatedListing { id })))
}

async fn lock_status(tx: &mut Tx, id: Uuid, org_id: Option<Uuid>) -> ApiResult<ListingStatus> {
    let row: Option<(String, Uuid)> =
        sqlx::query_as("SELECT status, org_id FROM product_listings WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
    let (status, owner) = row.ok_or(ApiError::NotFound)?;
    if org_id.is_some_and(|o| o != owner) {
        return Err(ApiError::NotFound);
    }
    ListingStatus::from_code(&status)
        .ok_or_else(|| ApiError::internal(std::io::Error::other("bad listing status")))
}

fn transition(from: ListingStatus, action: Action, actor: Actor) -> ApiResult<ListingStatus> {
    listing::next(from, action, actor).map_err(|_| ApiError::Conflict("invalid_transition"))
}

/// Any supplier change: the listing returns to draft (unpublished until reviewed again).
async fn mark_edited(tx: &mut Tx, id: Uuid, from: ListingStatus) -> ApiResult<()> {
    transition(from, Action::Edit, Actor::Supplier)?;
    sqlx::query(
        "UPDATE product_listings SET status = 'draft', published_at = NULL, submitted_at = NULL,
             updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[utoipa::path(
    operation_id = "listing_get_own",
    get,
    path = "/api/v1/orgs/{org_id}/listings/{id}",
    tag = "listings",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Own listing with media, review note and what is missing", body = ListingDetail),
        (status = 404, description = "Unknown listing", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn get_own(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<ListingDetail>> {
    let mut tx = supplier_tx(&state, &p, org_id).await?;
    let d = load_detail(&mut tx, id, false).await?;
    if d.summary.org_id != org_id {
        return Err(ApiError::NotFound);
    }
    tx.commit().await?;
    Ok(Json(d))
}

#[utoipa::path(
    operation_id = "listing_update",
    put,
    path = "/api/v1/orgs/{org_id}/listings/{id}",
    tag = "listings",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = ListingInput,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Saved; the listing is a draft again until resubmitted and approved"),
        (status = 409, description = "Retired listings cannot change", body = ErrorBody),
        (status = 422, description = "Invalid input", body = ErrorBody),
        (status = 404, description = "Unknown listing", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn update(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<ListingInput>,
) -> ApiResult<StatusCode> {
    let c = clean(&body)?;
    let mut tx = supplier_tx(&state, &p, org_id).await?;
    let from = lock_status(&mut tx, id, Some(org_id)).await?;
    mark_edited(&mut tx, id, from).await?;
    sqlx::query(
        "UPDATE product_listings SET manufacturer = $2, model_number = $3, model_key = $4,
             serial_number = $5, condition = $6, title = $7, title_en = $8, description = $9,
             description_en = $10, specs = $11, currency = $12, business_price_minor = $13,
             consumer_price_minor = $14, tax_rate_bps = $15, stock = $16, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(&c.manufacturer)
    .bind(&c.model_number)
    .bind(listing::normalise_model(&c.model_number))
    .bind(&c.serial_number)
    .bind(&c.condition)
    .bind(&c.title)
    .bind(&c.title_en)
    .bind(&c.description)
    .bind(&c.description_en)
    .bind(&c.specs)
    .bind(&c.currency)
    .bind(c.business)
    .bind(c.consumer)
    .bind(c.tax_rate_bps)
    .bind(c.stock)
    .execute(&mut *tx)
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "listing.updated",
            "listing",
            id,
            json!({ "from": from.code(), "title": c.title }),
        )
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    operation_id = "listing_delete_draft",
    delete,
    path = "/api/v1/orgs/{org_id}/listings/{id}",
    tag = "listings",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Draft deleted (published listings are retired instead)"),
        (status = 409, description = "Not a draft", body = ErrorBody),
        (status = 404, description = "Unknown listing", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn delete_draft(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    let mut tx = supplier_tx(&state, &p, org_id).await?;
    let from = lock_status(&mut tx, id, Some(org_id)).await?;
    if from != ListingStatus::Draft {
        return Err(ApiError::Conflict("not_draft"));
    }
    // Was it ever reviewed? Then it stays on record (retire it instead).
    let reviewed: bool =
        sqlx::query_scalar("SELECT reviewed_at IS NOT NULL FROM product_listings WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if reviewed {
        return Err(ApiError::Conflict("not_draft"));
    }
    sqlx::query("DELETE FROM listing_media WHERE listing_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM product_listings WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    Audit { org_id, p: &p }
        .record(&mut tx, "listing.deleted", "listing", id, json!({}))
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    operation_id = "listing_submit",
    post,
    path = "/api/v1/orgs/{org_id}/listings/{id}/submit",
    tag = "listings",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Submitted for Chuo-Sogo's review"),
        (status = 422, description = "Incomplete (see `missing` in the listing)", body = ErrorBody),
        (status = 409, description = "Not a draft", body = ErrorBody),
        (status = 404, description = "Unknown listing", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn submit(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    let mut tx = supplier_tx(&state, &p, org_id).await?;
    let from = lock_status(&mut tx, id, Some(org_id)).await?;
    transition(from, Action::Submit, Actor::Supplier)?;
    let d = load_detail(&mut tx, id, false).await?;
    if !d.missing.is_empty() {
        return Err(ApiError::Validation("listing_incomplete"));
    }
    sqlx::query(
        "UPDATE product_listings SET status = 'submitted', submitted_at = now(), updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    Audit { org_id, p: &p }
        .record(&mut tx, "listing.submitted", "listing", id, json!({}))
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    operation_id = "listing_retire_own",
    post,
    path = "/api/v1/orgs/{org_id}/listings/{id}/retire",
    tag = "listings",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Retired (removed from the catalogue for good)"),
        (status = 409, description = "Already retired", body = ErrorBody),
        (status = 404, description = "Unknown listing", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn retire_own(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    let mut tx = supplier_tx(&state, &p, org_id).await?;
    let from = lock_status(&mut tx, id, Some(org_id)).await?;
    transition(from, Action::Retire, Actor::Supplier)?;
    sqlx::query(
        "UPDATE product_listings SET status = 'retired', published_at = NULL, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "listing.retired",
            "listing",
            id,
            json!({ "from": from.code() }),
        )
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct MediaUpload {
    /// `photo` (JPEG/PNG/WebP, at least 1200 px on the long side) or `document` (PDF).
    pub kind: String,
    pub file_name: String,
    /// File content, standard base64.
    pub data_base64: String,
}

#[utoipa::path(
    operation_id = "listing_upload_media",
    post,
    path = "/api/v1/orgs/{org_id}/listings/{id}/media",
    tag = "listings",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = MediaUpload,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Stored (photos without metadata); the listing is a draft again", body = CreatedListing),
        (status = 422, description = "Wrong type, too small, or too many files", body = ErrorBody),
        (status = 409, description = "Retired listing", body = ErrorBody),
        (status = 413, description = "Larger than 10 MB"),
        (status = 404, description = "Unknown listing", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn upload_media(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<MediaUpload>,
) -> ApiResult<(StatusCode, Json<CreatedListing>)> {
    let photo = match body.kind.as_str() {
        "photo" => true,
        "document" => false,
        _ => return Err(ApiError::Validation("kind")),
    };
    let file_name =
        storage::clean_file_name(&body.file_name).ok_or(ApiError::Validation("file_name"))?;
    let access = p.access(&state, org_id).await?;
    if !matches!(access, Access::Member(_)) {
        return Err(ApiError::NotFound);
    }
    common::require_perm(access, Permission::ManageListings)?;
    let _permit = state
        .upload_permits
        .acquire()
        .await
        .map_err(ApiError::internal)?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(body.data_base64.as_bytes())
        .map_err(|_| ApiError::BadRequest("invalid_base64"))?;
    if raw.is_empty() || raw.len() > MAX_ATTACHMENT_BYTES {
        return Err(ApiError::Validation("file_size"));
    }
    let content_type =
        storage::content_type_for(&file_name, &raw).ok_or(ApiError::Validation("file_type"))?;
    let (bytes, dims) = if photo {
        if !content_type.starts_with("image/") {
            return Err(ApiError::Validation("file_type"));
        }
        let (w, h) = media::dimensions(&raw).ok_or(ApiError::Validation("file_type"))?;
        if !listing::photo_acceptable(w, h) {
            return Err(ApiError::Validation("photo_too_small"));
        }
        let clean =
            media::strip_metadata(content_type, &raw).ok_or(ApiError::Validation("file_type"))?;
        (clean, Some((w, h)))
    } else {
        if content_type != "application/pdf" {
            return Err(ApiError::Validation("file_type"));
        }
        (raw, None)
    };

    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let from = lock_status(&mut tx, id, Some(org_id)).await?;
    let limit = i32::try_from(if photo { MAX_PHOTOS } else { MAX_DOCUMENTS })
        .map_err(ApiError::internal)?;
    // The lowest free slot (1 = cover); none free means the limit is reached.
    let position: Option<i32> = sqlx::query_scalar(
        "SELECT min(p) FROM generate_series(1, $3) AS p
         WHERE p NOT IN (SELECT position FROM listing_media WHERE listing_id = $1 AND kind = $2)",
    )
    .bind(id)
    .bind(&body.kind)
    .bind(limit)
    .fetch_one(&mut *tx)
    .await?;
    let position = position.ok_or(ApiError::Validation("too_many_files"))?;
    mark_edited(&mut tx, id, from).await?;
    let hash = state
        .storage
        .put(&bytes)
        .await
        .map_err(ApiError::internal)?;
    let media_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO listing_media (id, org_id, listing_id, kind, position, file_name, content_type,
                                    size_bytes, sha256, width, height, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(media_id)
    .bind(org_id)
    .bind(id)
    .bind(&body.kind)
    .bind(position)
    .bind(&file_name)
    .bind(content_type)
    .bind(i64::try_from(bytes.len()).map_err(|_| ApiError::Validation("file_size"))?)
    .bind(hash.as_slice())
    .bind(dims.map(|(w, _)| i32::try_from(w).unwrap_or(i32::MAX)))
    .bind(dims.map(|(_, h)| i32::try_from(h).unwrap_or(i32::MAX)))
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "listing.media_added",
            "listing",
            id,
            json!({ "media": media_id, "kind": body.kind, "file_name": file_name }),
        )
        .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(CreatedListing { id: media_id })))
}

#[utoipa::path(
    operation_id = "listing_delete_media",
    delete,
    path = "/api/v1/orgs/{org_id}/listings/{id}/media/{media_id}",
    tag = "listings",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path), ("media_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Removed; the listing is a draft again"),
        (status = 409, description = "Retired listing", body = ErrorBody),
        (status = 404, description = "Unknown file", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn delete_media(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id, media_id)): ApiPath<(Uuid, Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    let mut tx = supplier_tx(&state, &p, org_id).await?;
    let from = lock_status(&mut tx, id, Some(org_id)).await?;
    mark_edited(&mut tx, id, from).await?;
    let gone = sqlx::query("DELETE FROM listing_media WHERE id = $1 AND listing_id = $2")
        .bind(media_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    if gone.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "listing.media_removed",
            "listing",
            id,
            json!({ "media": media_id }),
        )
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The stored file of one listing's media item, visible under the caller's transaction.
async fn media_response(
    state: &AppState,
    mut tx: Tx,
    id: Uuid,
    media_id: Uuid,
) -> ApiResult<impl IntoResponse + use<>> {
    let row: Option<(String, Vec<u8>)> = sqlx::query_as(
        "SELECT content_type, sha256 FROM listing_media WHERE id = $1 AND listing_id = $2",
    )
    .bind(media_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    let (content_type, sha) = row.ok_or(ApiError::NotFound)?;
    let hash: [u8; 32] = sha
        .as_slice()
        .try_into()
        .map_err(|_| ApiError::internal(std::io::Error::other("bad stored hash")))?;
    let bytes = state.storage.get(&hash).await.map_err(ApiError::internal)?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&content_type).map_err(ApiError::internal)?,
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok((headers, bytes))
}

#[utoipa::path(
    operation_id = "listing_own_media",
    get,
    path = "/api/v1/orgs/{org_id}/listings/{id}/media/{media_id}/content",
    tag = "listings",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path), ("media_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Photo or document bytes"),
        (status = 404, description = "Unknown file", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn own_media(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id, media_id)): ApiPath<(Uuid, Uuid, Uuid)>,
) -> ApiResult<impl IntoResponse> {
    let mut tx = supplier_tx(&state, &p, org_id).await?;
    lock_status(&mut tx, id, Some(org_id)).await?;
    media_response(&state, tx, id, media_id).await
}

// ---------------------------------------------------------------------------------------
// Moderation (admin console).

async fn moderation_tx(state: &AppState, p: &Principal) -> ApiResult<(Tx, Uuid)> {
    let platform = p.require_platform(Permission::ModerateListings)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, platform).await?;
    db::enable_platform_scope(&mut tx).await?;
    Ok((tx, platform))
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct ModerationQuery {
    /// Default `submitted` (the review queue).
    pub status: Option<String>,
}

#[utoipa::path(
    operation_id = "admin_list_listings",
    get,
    path = "/api/admin/v1/listings",
    tag = "listings",
    params(ModerationQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Listings in a state (oldest submission first)", body = [ListingSummary]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn moderation_list(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiQuery(q): ApiQuery<ModerationQuery>,
) -> ApiResult<Json<Vec<ListingSummary>>> {
    let status = q.status.unwrap_or_else(|| "submitted".into());
    if ListingStatus::from_code(&status).is_none() {
        return Err(ApiError::Validation("status"));
    }
    let (mut tx, _) = moderation_tx(&state, &p).await?;
    let rows = sqlx::query_as::<_, ListingSummary>(&format!(
        "{SUMMARY_SELECT} WHERE l.status = $1
         ORDER BY l.submitted_at NULLS LAST, l.updated_at DESC LIMIT 500"
    ))
    .bind(&status)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[utoipa::path(
    operation_id = "admin_get_listing",
    get,
    path = "/api/admin/v1/listings/{id}",
    tag = "listings",
    params(("id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Listing with media and possible duplicates", body = ListingDetail),
        (status = 404, description = "Unknown listing", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn moderation_get(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<Json<ListingDetail>> {
    let (mut tx, _) = moderation_tx(&state, &p).await?;
    let d = load_detail(&mut tx, id, true).await?;
    tx.commit().await?;
    Ok(Json(d))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ReviewBody {
    /// Required for a rejection (shown to the supplier); optional otherwise.
    #[serde(default)]
    pub note: String,
}

async fn review(
    state: &AppState,
    p: &Principal,
    id: Uuid,
    action: Action,
    note: &str,
) -> ApiResult<()> {
    let note = common::text(note, 2000, true, "note")?;
    if action == Action::Reject && note.is_empty() {
        return Err(ApiError::Validation("note"));
    }
    let (mut tx, platform) = moderation_tx(state, p).await?;
    let from = lock_status(&mut tx, id, None).await?;
    let to = transition(from, action, Actor::Moderator)?;
    let owner: Uuid = sqlx::query_scalar("SELECT org_id FROM product_listings WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE product_listings SET status = $2, review_note = $3, reviewed_by = $4,
             reviewed_at = now(),
             published_at = CASE WHEN $2 = 'approved' THEN now() ELSE NULL END,
             updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(to.code())
    .bind(&note)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    let action_code = match to {
        ListingStatus::Approved => "listing.approved",
        ListingStatus::Rejected => "listing.rejected",
        _ => "listing.retired",
    };
    for target in [platform, owner] {
        Audit { org_id: target, p }
            .record(
                &mut tx,
                action_code,
                "listing",
                id,
                json!({ "from": from.code(), "note": note }),
            )
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

macro_rules! review_endpoint {
    ($name:ident, $path:literal, $op:literal, $action:expr, $desc:literal) => {
        #[utoipa::path(
            operation_id = $op,
            post,
            path = $path,
            tag = "listings",
            params(("id" = Uuid, Path)),
            request_body = ReviewBody,
            security(("bearer" = [])),
            responses(
                (status = 204, description = $desc),
                (status = 409, description = "Not in a state that allows this", body = ErrorBody),
                (status = 422, description = "A rejection needs a note", body = ErrorBody),
                (status = 404, description = "Unknown listing", body = ErrorBody),
                (status = 403, description = "Not permitted", body = ErrorBody),
                (status = 401, description = "No valid admin session", body = ErrorBody),
            )
        )]
        pub async fn $name(
            State(state): State<AppState>,
            AdminPrincipal(p): AdminPrincipal,
            ApiPath(id): ApiPath<Uuid>,
            ApiJson(body): ApiJson<ReviewBody>,
        ) -> ApiResult<StatusCode> {
            review(&state, &p, id, $action, &body.note).await?;
            Ok(StatusCode::NO_CONTENT)
        }
    };
}

review_endpoint!(
    approve,
    "/api/admin/v1/listings/{id}/approve",
    "admin_approve_listing",
    Action::Approve,
    "Approved and published in the catalogue"
);
review_endpoint!(
    reject,
    "/api/admin/v1/listings/{id}/reject",
    "admin_reject_listing",
    Action::Reject,
    "Rejected with a note for the supplier"
);
review_endpoint!(
    retire,
    "/api/admin/v1/listings/{id}/retire",
    "admin_retire_listing",
    Action::Retire,
    "Removed from the catalogue for good"
);

#[utoipa::path(
    operation_id = "admin_listing_media",
    get,
    path = "/api/admin/v1/listings/{id}/media/{media_id}/content",
    tag = "listings",
    params(("id" = Uuid, Path), ("media_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Photo or document bytes"),
        (status = 404, description = "Unknown file", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn moderation_media(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath((id, media_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<impl IntoResponse> {
    let (tx, _) = moderation_tx(&state, &p).await?;
    media_response(&state, tx, id, media_id).await
}

// ---------------------------------------------------------------------------------------
// Catalogue (published listings).

async fn catalogue_tx(state: &AppState, p: &Principal, org_id: Uuid) -> ApiResult<Tx> {
    let access = p.access(state, org_id).await?;
    if !matches!(access, Access::Member(_)) {
        return Err(ApiError::NotFound);
    }
    common::require_perm(access, Permission::ViewCatalogue)?;
    Ok(db::tenant_tx(&state.pool, p.user_id, org_id).await?)
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct CatalogueQuery {
    /// Words in title, manufacturer or model number.
    pub q: Option<String>,
}

/// A suspended supplier's listings disappear from the catalogue (and come back on reactivation).
async fn supplier_active(tx: &mut Tx, listing_id: Uuid) -> ApiResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM product_listings l JOIN organisations o ON o.id = l.org_id
                        WHERE l.id = $1 AND o.status = 'active')",
    )
    .bind(listing_id)
    .fetch_one(&mut **tx)
    .await?)
}

#[utoipa::path(
    operation_id = "listing_catalogue",
    get,
    path = "/api/v1/orgs/{org_id}/catalogue",
    tag = "listings",
    params(("org_id" = Uuid, Path), CatalogueQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Published listings (at most 200)", body = [ListingSummary]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn catalogue(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiQuery(q): ApiQuery<CatalogueQuery>,
) -> ApiResult<Json<Vec<ListingSummary>>> {
    let search = common::text(q.q.as_deref().unwrap_or(""), 100, false, "q")?;
    let model_key = listing::normalise_model(&search);
    let mut tx = catalogue_tx(&state, &p, org_id).await?;
    // Literal search: escape LIKE wildcards in the user's text.
    let pattern = format!(
        "%{}%",
        search
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let rows = sqlx::query_as::<_, ListingSummary>(&format!(
        "{SUMMARY_SELECT} WHERE l.status = 'approved' AND o.status = 'active'
           AND ($1 = '' OR l.title ILIKE $2 OR l.title_en ILIKE $2 OR l.manufacturer ILIKE $2
                OR ($3 <> '' AND l.model_key LIKE '%' || $3 || '%'))
         ORDER BY l.published_at DESC LIMIT 200"
    ))
    .bind(&search)
    .bind(&pattern)
    .bind(&model_key)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[utoipa::path(
    operation_id = "listing_catalogue_item",
    get,
    path = "/api/v1/orgs/{org_id}/catalogue/{id}",
    tag = "listings",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Published listing", body = ListingDetail),
        (status = 404, description = "Unknown or not published", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn catalogue_item(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<ListingDetail>> {
    let mut tx = catalogue_tx(&state, &p, org_id).await?;
    let mut d = load_detail(&mut tx, id, false).await?;
    if d.summary.status != "approved" || !supplier_active(&mut tx, id).await? {
        return Err(ApiError::NotFound);
    }
    tx.commit().await?;
    d.review_note.clear();
    d.missing.clear();
    Ok(Json(d))
}

#[utoipa::path(
    operation_id = "listing_catalogue_media",
    get,
    path = "/api/v1/orgs/{org_id}/catalogue/{id}/media/{media_id}/content",
    tag = "listings",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path), ("media_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Photo or document bytes of a published listing"),
        (status = 404, description = "Unknown or not published", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn catalogue_media(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id, media_id)): ApiPath<(Uuid, Uuid, Uuid)>,
) -> ApiResult<impl IntoResponse> {
    let mut tx = catalogue_tx(&state, &p, org_id).await?;
    let published: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM product_listings WHERE id = $1 AND status = 'approved')",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if !published || !supplier_active(&mut tx, id).await? {
        return Err(ApiError::NotFound);
    }
    media_response(&state, tx, id, media_id).await
}
