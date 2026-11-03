//! Procurement requests and their items: customer drafting/submission, operator intake,
//! clarification, human product verification and the move to sourcing.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, NaiveDate, Utc};
use procure_domain::rbac::Permission;
use procure_domain::workflow::{RequestStatus, Side};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::api::common::{
    self, Audit, RequestRow, View, lock_request, request_view, require_perm, side, transition,
};
use crate::db;
use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ApiResult, ErrorBody};
use crate::principal::Principal;

pub const MAX_ITEMS: usize = 200;

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ItemInput {
    pub description: String,
    #[serde(default)]
    pub manufacturer: String,
    /// Manufacturer part number as the customer knows it.
    #[serde(default)]
    pub mpn: String,
    pub quantity: u32,
    /// Unit code, e.g. `pcs`, `box`, `m` (lowercase letters and `_`).
    #[serde(default = "default_unit")]
    pub unit: String,
    #[serde(default)]
    pub notes: String,
}

fn default_unit() -> String {
    "pcs".into()
}

struct CleanItem {
    description: String,
    manufacturer: String,
    mpn: String,
    quantity: i32,
    unit: String,
    notes: String,
}

fn clean_item(i: &ItemInput) -> ApiResult<CleanItem> {
    let unit = i.unit.trim();
    if unit.is_empty()
        || unit.len() > 16
        || !unit.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
    {
        return Err(ApiError::Validation("unit"));
    }
    if i.quantity == 0 || i.quantity > 1_000_000_000 {
        return Err(ApiError::Validation("quantity"));
    }
    Ok(CleanItem {
        description: common::required(&i.description, 1000, true, "description")?,
        manufacturer: common::text(&i.manufacturer, 200, false, "manufacturer")?,
        mpn: common::text(&i.mpn, 100, false, "mpn")?,
        quantity: i32::try_from(i.quantity).map_err(|_| ApiError::Validation("quantity"))?,
        unit: unit.to_string(),
        notes: common::text(&i.notes, 2000, true, "notes")?,
    })
}

async fn insert_item(
    tx: &mut db::Tx,
    org_id: Uuid,
    request_id: Uuid,
    item: &CleanItem,
) -> ApiResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO request_items (id, org_id, request_id, line_no, description, manufacturer, mpn,
                                    quantity, unit, notes)
         VALUES ($1, $2, $3,
                 (SELECT coalesce(max(line_no), 0) + 1 FROM request_items WHERE request_id = $3),
                 $4, $5, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(org_id)
    .bind(request_id)
    .bind(&item.description)
    .bind(&item.manufacturer)
    .bind(&item.mpn)
    .bind(item.quantity)
    .bind(&item.unit)
    .bind(&item.notes)
    .execute(&mut **tx)
    .await?;
    Ok(id)
}

async fn item_count(tx: &mut db::Tx, request_id: Uuid) -> ApiResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM request_items WHERE request_id = $1")
            .bind(request_id)
            .fetch_one(&mut **tx)
            .await?,
    )
}

/// The customer who raised a request edits it, and only while the workflow allows.
async fn lock_own_editable(
    tx: &mut db::Tx,
    p: &Principal,
    view: View,
    id: Uuid,
) -> ApiResult<RequestRow> {
    let req = lock_request(tx, id, view).await?;
    if req.requested_by != p.user_id {
        return Err(ApiError::Forbidden);
    }
    if !req.status()?.items_editable() {
        return Err(ApiError::Conflict("not_editable"));
    }
    Ok(req)
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreateRequest {
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// ISO 4217: JPY, USD, EUR, INR, CNY, KRW.
    pub currency: String,
    pub needed_by: Option<NaiveDate>,
    #[serde(default)]
    pub delivery_location: String,
    #[serde(default)]
    pub items: Vec<ItemInput>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct Created {
    pub id: Uuid,
    pub number: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/requests",
    tag = "requests",
    params(("org_id" = Uuid, Path)),
    request_body = CreateRequest,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Draft request created", body = Created),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 422, description = "Invalid input", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn create_request(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<CreateRequest>,
) -> ApiResult<(StatusCode, Json<Created>)> {
    p.require(&state, org_id, Permission::SubmitRequest).await?;
    let title = common::required(&req.title, 200, false, "title")?;
    let description = common::text(&req.description, 4000, true, "description")?;
    let location = common::text(&req.delivery_location, 500, true, "delivery_location")?;
    let currency = common::currency(&req.currency)?;
    if req.items.len() > MAX_ITEMS {
        return Err(ApiError::Validation("too_many_items"));
    }
    let items = req
        .items
        .iter()
        .map(clean_item)
        .collect::<ApiResult<Vec<_>>>()?;

    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let id = Uuid::now_v7();
    let number: String = sqlx::query_scalar("SELECT next_number($1, 'PR')")
        .bind(org_id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO procurement_requests (id, org_id, number, title, description, currency,
                                           needed_by, delivery_location, requested_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(org_id)
    .bind(&number)
    .bind(&title)
    .bind(&description)
    .bind(currency.code())
    .bind(req.needed_by)
    .bind(&location)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    for item in &items {
        insert_item(&mut tx, org_id, id, item).await?;
    }
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "request.created",
            "procurement_request",
            id,
            json!({ "number": number, "items": items.len(), "currency": currency.code() }),
        )
        .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(Created { id, number })))
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct RequestSummary {
    pub id: Uuid,
    pub number: String,
    pub title: String,
    pub status: String,
    pub currency: String,
    pub needed_by: Option<NaiveDate>,
    pub requested_by: Uuid,
    pub requester_name: Option<String>,
    pub item_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct ListQuery {
    /// Filter by workflow status code.
    pub status: Option<String>,
    /// `orders` (approved to closed) or `finance` (delivered to reconciled).
    pub stage: Option<String>,
    pub limit: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/requests",
    tag = "requests",
    params(("org_id" = Uuid, Path), ListQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Requests visible to the caller, newest first", body = [RequestSummary]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 422, description = "Unknown status filter", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list_requests(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> ApiResult<Json<Vec<RequestSummary>>> {
    let (_, view) = request_view(&p, &state, org_id).await?;
    if let Some(s) = &q.status
        && RequestStatus::from_code(s).is_none()
    {
        return Err(ApiError::Validation("status"));
    }
    let stage: Option<Vec<&str>> = match q.stage.as_deref() {
        None => None,
        Some("orders") => Some(vec![
            "approved",
            "ordered",
            "delivered",
            "invoiced",
            "disputed",
            "reconciled",
            "closed",
        ]),
        Some("finance") => Some(vec!["delivered", "invoiced", "disputed", "reconciled"]),
        Some(_) => return Err(ApiError::Validation("stage")),
    };
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let rows = sqlx::query_as::<_, RequestSummary>(
        "SELECT r.id, r.number, r.title, r.status, r.currency, r.needed_by, r.requested_by,
                u.display_name AS requester_name,
                (SELECT count(*) FROM request_items i WHERE i.request_id = r.id) AS item_count,
                r.created_at, r.updated_at
         FROM procurement_requests r LEFT JOIN users u ON u.id = r.requested_by
         WHERE ($1::uuid IS NULL OR r.requested_by = $1) AND ($2::text IS NULL OR r.status = $2)
           AND ($4::text[] IS NULL OR r.status = ANY($4))
         ORDER BY r.created_at DESC, r.id DESC LIMIT $3",
    )
    .bind(view.owner_filter())
    .bind(q.status)
    .bind(limit)
    .bind(stage)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct ItemView {
    pub id: Uuid,
    pub line_no: i32,
    pub description: String,
    pub manufacturer: String,
    pub mpn: String,
    pub quantity: i32,
    pub unit: String,
    pub notes: String,
    /// `unverified` or `verified` (by a human operator).
    pub verification_status: String,
    pub verified_manufacturer: Option<String>,
    pub verified_mpn: Option<String>,
    pub verification_evidence: Option<String>,
    pub verified_at: Option<DateTime<Utc>>,
    /// The catalogue record the item was verified against.
    pub product_identity_id: Option<Uuid>,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct RfqSummary {
    pub id: Uuid,
    pub number: String,
    pub status: String,
    pub response_due: Option<NaiveDate>,
    pub recipients: i64,
    pub quotations: i64,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct RecommendationSummary {
    pub id: Uuid,
    pub status: String,
    pub currency: String,
    pub total_minor: i64,
    pub supplier_name: Option<String>,
    pub required_approvals: Option<i32>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct RequestHeader {
    pub id: Uuid,
    pub number: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub currency: String,
    pub needed_by: Option<NaiveDate>,
    pub delivery_location: String,
    pub clarification_question: Option<String>,
    pub clarification_answer: Option<String>,
    pub requested_by: Uuid,
    pub requester_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub submitted_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct RequestDetail {
    pub request: RequestHeader,
    pub items: Vec<ItemView>,
    pub rfqs: Vec<RfqSummary>,
    /// The latest recommendation, if any.
    pub recommendation: Option<RecommendationSummary>,
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}",
    tag = "requests",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Request with items, RFQs and latest recommendation", body = RequestDetail),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or not visible", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn get_request(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<RequestDetail>> {
    let (access, view) = request_view(&p, &state, org_id).await?;
    // Customers see a recommendation only once Chuo-Sogo has sent it.
    let include_unsent = side(access)? == Side::Operator;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let request = sqlx::query_as::<_, RequestHeader>(
        "SELECT r.id, r.number, r.title, r.description, r.status, r.currency, r.needed_by,
                r.delivery_location, r.clarification_question, r.clarification_answer,
                r.requested_by, u.display_name AS requester_name, r.created_at, r.updated_at,
                r.submitted_at
         FROM procurement_requests r LEFT JOIN users u ON u.id = r.requested_by
         WHERE r.id = $1 AND ($2::uuid IS NULL OR r.requested_by = $2)",
    )
    .bind(request_id)
    .bind(view.owner_filter())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    let items = sqlx::query_as::<_, ItemView>(
        "SELECT id, line_no, description, manufacturer, mpn, quantity, unit, notes,
                verification_status, verified_manufacturer, verified_mpn, verification_evidence,
                verified_at, product_identity_id
         FROM request_items WHERE request_id = $1 ORDER BY line_no",
    )
    .bind(request_id)
    .fetch_all(&mut *tx)
    .await?;
    let rfqs = sqlx::query_as::<_, RfqSummary>(
        "SELECT f.id, f.number, f.status, f.response_due,
                (SELECT count(*) FROM rfq_recipients x WHERE x.rfq_id = f.id) AS recipients,
                (SELECT count(*) FROM quotations q WHERE q.rfq_id = f.id AND q.superseded_at IS NULL)
                    AS quotations
         FROM rfqs f WHERE f.request_id = $1 ORDER BY f.created_at",
    )
    .bind(request_id)
    .fetch_all(&mut *tx)
    .await?;
    let recommendation = sqlx::query_as::<_, RecommendationSummary>(
        "SELECT rc.id, rc.status, rc.currency, rc.total_minor, s.name AS supplier_name,
                rc.required_approvals, rc.created_at
         FROM recommendations rc
         JOIN quotations q ON q.id = rc.quotation_id
         JOIN rfq_recipients x ON x.id = q.rfq_recipient_id
         LEFT JOIN suppliers s ON s.id = x.supplier_id
         WHERE rc.request_id = $1 AND ($2 OR rc.sent_at IS NOT NULL)
         ORDER BY rc.created_at DESC LIMIT 1",
    )
    .bind(request_id)
    .bind(include_unsent)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(RequestDetail {
        request,
        items,
        rfqs,
        recommendation,
    }))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct UpdateRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    pub currency: Option<String>,
    pub needed_by: Option<NaiveDate>,
    pub delivery_location: Option<String>,
}

#[utoipa::path(
    patch,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}",
    tag = "requests",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    request_body = UpdateRequest,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Updated"),
        (status = 403, description = "Not the requester", body = ErrorBody),
        (status = 404, description = "Unknown or not visible", body = ErrorBody),
        (status = 409, description = "No longer editable", body = ErrorBody),
        (status = 422, description = "Invalid input", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn update_request(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(req): ApiJson<UpdateRequest>,
) -> ApiResult<StatusCode> {
    let (access, view) = request_view(&p, &state, org_id).await?;
    require_perm(access, Permission::SubmitRequest)?;
    let title = req
        .title
        .as_deref()
        .map(|t| common::required(t, 200, false, "title"))
        .transpose()?;
    let description = req
        .description
        .as_deref()
        .map(|t| common::text(t, 4000, true, "description"))
        .transpose()?;
    let location = req
        .delivery_location
        .as_deref()
        .map(|t| common::text(t, 500, true, "delivery_location"))
        .transpose()?;
    let currency = req.currency.as_deref().map(common::currency).transpose()?;

    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let row = lock_own_editable(&mut tx, &p, view, request_id).await?;
    sqlx::query(
        "UPDATE procurement_requests SET
             title = coalesce($2, title), description = coalesce($3, description),
             currency = coalesce($4, currency), needed_by = coalesce($5, needed_by),
             delivery_location = coalesce($6, delivery_location), updated_at = now()
         WHERE id = $1",
    )
    .bind(row.id)
    .bind(&title)
    .bind(&description)
    .bind(currency.map(|c| c.code()))
    .bind(req.needed_by)
    .bind(&location)
    .execute(&mut *tx)
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "request.updated",
            "procurement_request",
            row.id,
            json!({ "number": row.number, "currency": currency.map(|c| c.code()) }),
        )
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/items",
    tag = "requests",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    request_body = ItemInput,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Item added", body = Created),
        (status = 403, description = "Not the requester", body = ErrorBody),
        (status = 404, description = "Unknown or not visible", body = ErrorBody),
        (status = 409, description = "No longer editable", body = ErrorBody),
        (status = 422, description = "Invalid input", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn add_item(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(item): ApiJson<ItemInput>,
) -> ApiResult<(StatusCode, Json<Created>)> {
    let (access, view) = request_view(&p, &state, org_id).await?;
    require_perm(access, Permission::SubmitRequest)?;
    let item = clean_item(&item)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let row = lock_own_editable(&mut tx, &p, view, request_id).await?;
    if item_count(&mut tx, row.id).await? >= MAX_ITEMS as i64 {
        return Err(ApiError::Validation("too_many_items"));
    }
    let id = insert_item(&mut tx, org_id, row.id, &item).await?;
    let line_no: i32 = sqlx::query_scalar("SELECT line_no FROM request_items WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "item.added",
            "request_item",
            id,
            json!({ "request": row.id, "number": row.number, "line_no": line_no }),
        )
        .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(Created {
            id,
            number: format!("{}/{line_no}", row.number),
        }),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/items/{item_id}",
    tag = "requests",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path), ("item_id" = Uuid, Path)),
    request_body = ItemInput,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Item replaced; any previous verification is cleared"),
        (status = 403, description = "Not the requester", body = ErrorBody),
        (status = 404, description = "Unknown or not visible", body = ErrorBody),
        (status = 409, description = "No longer editable", body = ErrorBody),
        (status = 422, description = "Invalid input", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn replace_item(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id, item_id)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(item): ApiJson<ItemInput>,
) -> ApiResult<StatusCode> {
    let (access, view) = request_view(&p, &state, org_id).await?;
    require_perm(access, Permission::SubmitRequest)?;
    let item = clean_item(&item)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let row = lock_own_editable(&mut tx, &p, view, request_id).await?;
    let updated = sqlx::query(
        "UPDATE request_items SET description = $3, manufacturer = $4, mpn = $5, quantity = $6,
                unit = $7, notes = $8, verification_status = 'unverified', product_identity_id = NULL,
                verified_manufacturer = NULL, verified_mpn = NULL, verification_evidence = NULL,
                verified_by = NULL, verified_at = NULL
         WHERE id = $1 AND request_id = $2",
    )
    .bind(item_id)
    .bind(row.id)
    .bind(&item.description)
    .bind(&item.manufacturer)
    .bind(&item.mpn)
    .bind(item.quantity)
    .bind(&item.unit)
    .bind(&item.notes)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "item.updated",
            "request_item",
            item_id,
            json!({ "request": row.id, "number": row.number, "quantity": item.quantity }),
        )
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/items/{item_id}",
    tag = "requests",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path), ("item_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Item removed"),
        (status = 403, description = "Not the requester", body = ErrorBody),
        (status = 404, description = "Unknown or not visible", body = ErrorBody),
        (status = 409, description = "No longer editable, or already quoted", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn delete_item(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id, item_id)): ApiPath<(Uuid, Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    let (access, view) = request_view(&p, &state, org_id).await?;
    require_perm(access, Permission::SubmitRequest)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let row = lock_own_editable(&mut tx, &p, view, request_id).await?;
    let quoted: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM quotation_lines WHERE request_item_id = $1)",
    )
    .bind(item_id)
    .fetch_one(&mut *tx)
    .await?;
    if quoted {
        // Quotations reference it; the record of what was quoted must stay intact.
        return Err(ApiError::Conflict("item_quoted"));
    }
    // Files attached to this item stay on the request.
    let detached =
        sqlx::query("UPDATE attachments SET item_id = NULL WHERE item_id = $1 AND request_id = $2")
            .bind(item_id)
            .bind(row.id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    let deleted = sqlx::query("DELETE FROM request_items WHERE id = $1 AND request_id = $2")
        .bind(item_id)
        .bind(row.id)
        .execute(&mut *tx)
        .await?;
    if deleted.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "item.removed",
            "request_item",
            item_id,
            json!({ "request": row.id, "number": row.number, "attachments_detached": detached }),
        )
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, Default, utoipa::ToSchema)]
pub struct SubmitBody {
    /// Required when answering a clarification question.
    pub answer: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/submit",
    tag = "requests",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    request_body = SubmitBody,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Submitted to Chuo-Sogo"),
        (status = 403, description = "Not the requester", body = ErrorBody),
        (status = 404, description = "Unknown or not visible", body = ErrorBody),
        (status = 409, description = "Not in a submittable state", body = ErrorBody),
        (status = 422, description = "No items, or missing clarification answer", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn submit(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<SubmitBody>,
) -> ApiResult<StatusCode> {
    let (access, view) = request_view(&p, &state, org_id).await?;
    require_perm(access, Permission::SubmitRequest)?;
    let side = side(access)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let row = lock_own_editable(&mut tx, &p, view, request_id).await?;
    if item_count(&mut tx, row.id).await? == 0 {
        return Err(ApiError::Validation("no_items"));
    }
    let answer = if row.status()? == RequestStatus::NeedsClarification {
        let a = body.answer.as_deref().unwrap_or("");
        Some(common::required(a, 2000, true, "answer")?)
    } else {
        None
    };
    if let Some(a) = &answer {
        sqlx::query("UPDATE procurement_requests SET clarification_answer = $2 WHERE id = $1")
            .bind(row.id)
            .bind(a)
            .execute(&mut *tx)
            .await?;
    }
    let action = if answer.is_some() {
        "request.clarified"
    } else {
        "request.submitted"
    };
    transition(
        &mut tx,
        &Audit { org_id, p: &p },
        &row,
        RequestStatus::Submitted,
        side,
        action,
        json!({ "answer": answer }),
    )
    .await?;
    let identify_due = chrono::Utc::now()
        + chrono::Duration::hours(i64::from(
            crate::api::admin::load_settings(&mut tx, org_id)
                .await?
                .identify_sla_hours,
        ));
    // Chuo-Sogo's service level starts now: identify the products.
    crate::api::ops::open_auto_task(
        &mut tx,
        org_id,
        row.id,
        None,
        "identify",
        &format!("Identify products for {}", row.number),
        identify_due,
    )
    .await?;
    crate::api::ops::flag_unknown_items(&mut tx, org_id, row.id, &row.number, identify_due).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, Default, utoipa::ToSchema)]
pub struct ReasonBody {
    #[serde(default)]
    pub reason: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/cancel",
    tag = "requests",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    request_body = ReasonBody,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Cancelled"),
        (status = 403, description = "Not the requester", body = ErrorBody),
        (status = 404, description = "Unknown or not visible", body = ErrorBody),
        (status = 409, description = "Work has started; cannot cancel", body = ErrorBody),
        (status = 422, description = "Invalid input", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn cancel(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<ReasonBody>,
) -> ApiResult<StatusCode> {
    let (access, view) = request_view(&p, &state, org_id).await?;
    require_perm(access, Permission::SubmitRequest)?;
    let side = side(access)?;
    let reason = common::text(&body.reason, 2000, true, "reason")?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let row = lock_request(&mut tx, request_id, view).await?;
    if row.requested_by != p.user_id {
        return Err(ApiError::Forbidden);
    }
    transition(
        &mut tx,
        &Audit { org_id, p: &p },
        &row,
        RequestStatus::Cancelled,
        side,
        "request.cancelled",
        json!({ "reason": reason }),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Operator-side transition with no extra data.
async fn operator_step(
    state: &AppState,
    p: &Principal,
    org_id: Uuid,
    request_id: Uuid,
    perm: Permission,
    to: RequestStatus,
    action: &str,
) -> ApiResult<StatusCode> {
    let access = p.require(state, org_id, perm).await?;
    let side = side(access)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let row = lock_request(&mut tx, request_id, View::All).await?;
    if to == RequestStatus::Sourcing {
        let unverified: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM request_items
             WHERE request_id = $1 AND verification_status <> 'verified'",
        )
        .bind(row.id)
        .fetch_one(&mut *tx)
        .await?;
        if unverified > 0 {
            return Err(ApiError::Conflict("items_not_verified"));
        }
    }
    transition(
        &mut tx,
        &Audit { org_id, p },
        &row,
        to,
        side,
        action,
        json!({}),
    )
    .await?;
    if to == RequestStatus::Sourcing {
        crate::api::ops::close_auto_tasks(&mut tx, row.id, None, "identify").await?;
        crate::api::ops::close_auto_tasks(&mut tx, row.id, None, "unknown_item").await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/pick-up",
    tag = "requests",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Operator started product identification"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or not assigned", body = ErrorBody),
        (status = 409, description = "Not in a state that allows this", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn pick_up(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    operator_step(
        &state,
        &p,
        org_id,
        request_id,
        Permission::OperateRequests,
        RequestStatus::Identifying,
        "request.picked_up",
    )
    .await
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/start-sourcing",
    tag = "requests",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 204, description = "All items verified; sourcing started"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or not assigned", body = ErrorBody),
        (status = 409, description = "Items not verified, or wrong state", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn start_sourcing(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    operator_step(
        &state,
        &p,
        org_id,
        request_id,
        Permission::OperateRequests,
        RequestStatus::Sourcing,
        "request.sourcing_started",
    )
    .await
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ClarificationBody {
    pub question: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/clarification",
    tag = "requests",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    request_body = ClarificationBody,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Question sent back to the requester"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or not assigned", body = ErrorBody),
        (status = 409, description = "Not in a state that allows this", body = ErrorBody),
        (status = 422, description = "Missing question", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn request_clarification(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<ClarificationBody>,
) -> ApiResult<StatusCode> {
    let access = p
        .require(&state, org_id, Permission::OperateRequests)
        .await?;
    let side = side(access)?;
    let question = common::required(&body.question, 2000, true, "question")?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let row = lock_request(&mut tx, request_id, View::All).await?;
    sqlx::query(
        "UPDATE procurement_requests SET clarification_question = $2, clarification_answer = NULL
         WHERE id = $1",
    )
    .bind(row.id)
    .bind(&question)
    .execute(&mut *tx)
    .await?;
    transition(
        &mut tx,
        &Audit { org_id, p: &p },
        &row,
        RequestStatus::NeedsClarification,
        side,
        "request.clarification_requested",
        json!({ "question": question }),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct VerifyItemBody {
    /// A verified record from Chuo-Sogo's product catalogue (`/api/v1/products`).
    pub product_id: Uuid,
    /// Why this record matches the customer's item (optional).
    #[serde(default)]
    pub note: String,
}

#[derive(sqlx::FromRow)]
struct VerifiedProduct {
    manufacturer: String,
    mpn: String,
    status: String,
    evidence: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/items/{item_id}/verification",
    tag = "requests",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path), ("item_id" = Uuid, Path)),
    request_body = VerifyItemBody,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Item linked to a verified product record by the operator"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown item, product or not assigned", body = ErrorBody),
        (status = 409, description = "Request is not being identified, or product not verified", body = ErrorBody),
        (status = 422, description = "Invalid note", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn verify_item(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id, item_id)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(body): ApiJson<VerifyItemBody>,
) -> ApiResult<StatusCode> {
    let access = p
        .require(&state, org_id, Permission::VerifyProducts)
        .await?;
    if side(access)? != Side::Operator {
        return Err(ApiError::Forbidden);
    }
    let note = common::text(&body.note, 1000, true, "note")?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let row = lock_request(&mut tx, request_id, View::All).await?;
    if row.status()? != RequestStatus::Identifying {
        return Err(ApiError::Conflict("invalid_transition"));
    }
    // The catalogue is platform-level: read it in staff scope (caller is assigned staff).
    db::enable_staff_scope(&mut tx).await?;
    let product = sqlx::query_as::<_, VerifiedProduct>(
        "SELECT p.manufacturer, p.mpn, p.status,
                (SELECT string_agg(e.url, ' | ' ORDER BY e.created_at)
                 FROM product_evidence e
                 WHERE e.product_id = p.id AND e.url IS NOT NULL
                   AND e.kind <> 'supplier_confirmation') AS evidence
         FROM product_identities p WHERE p.id = $1 FOR SHARE",
    )
    .bind(body.product_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    if product.status != "verified" {
        return Err(ApiError::Conflict("product_not_verified"));
    }
    // Snapshot of what the decision rested on, kept with the item and shown to the customer:
    // the operator's note plus public sources only.
    let mut evidence = product.evidence.unwrap_or_default();
    if !note.is_empty() {
        evidence = format!("{note} | {evidence}");
    }
    let evidence: String = evidence.chars().take(2000).collect();
    let updated = sqlx::query(
        "UPDATE request_items SET verification_status = 'verified', product_identity_id = $3,
                verified_manufacturer = $4, verified_mpn = $5, verification_evidence = $6,
                verified_by = $7, verified_at = now()
         WHERE id = $1 AND request_id = $2",
    )
    .bind(item_id)
    .bind(row.id)
    .bind(body.product_id)
    .bind(&product.manufacturer)
    .bind(&product.mpn)
    .bind(&evidence)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "item.verified",
            "request_item",
            item_id,
            json!({ "request": row.id, "number": row.number, "product": body.product_id,
                    "manufacturer": product.manufacturer, "mpn": product.mpn, "note": note }),
        )
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------------------
// CSV exchange with the factory (CEO, 2026-11-03): purchase requests from its own file.

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ImportRequestsBody {
    /// `.csv`, `.tsv` (UTF-8 or Shift_JIS) or `.xlsx`. One row per line; rows with the same
    /// value in the request column (件名 / request) become one request.
    pub file_name: String,
    /// File content, standard base64.
    pub data_base64: String,
    /// ISO 4217; default JPY.
    pub currency: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct SkippedRow {
    /// 1-based row in the file.
    pub row: usize,
    /// `no_item`, `quantity`, `unit`, `needed_by`, `too_long`, `too_many_lines`,
    /// `too_many_requests`.
    pub reason: &'static str,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ImportRequestsResponse {
    /// Draft requests created (the requester reviews and submits each one).
    pub created: Vec<Created>,
    pub lines: usize,
    pub skipped: Vec<SkippedRow>,
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/requests/import",
    tag = "requests",
    params(("org_id" = Uuid, Path)),
    request_body = ImportRequestsBody,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Draft requests created from the file; rows that could not be read are listed", body = ImportRequestsResponse),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 422, description = "Unsupported or empty file, no header, or no readable row", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn import_requests(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<ImportRequestsBody>,
) -> ApiResult<(StatusCode, Json<ImportRequestsResponse>)> {
    use base64::Engine;
    use procure_domain::request_import;
    p.require(&state, org_id, Permission::SubmitRequest).await?;
    let file_name = common::required(&body.file_name, 255, false, "file_name")?;
    let currency = common::currency(body.currency.as_deref().unwrap_or("JPY"))?;
    // A file exchanged with the factory's systems: spreadsheets only, read exactly (scans
    // and free text go through the AI intake, which a person confirms).
    let ext = file_name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    if !matches!(ext.as_str(), "csv" | "tsv" | "txt" | "xlsx") {
        return Err(ApiError::Validation("unsupported"));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body.data_base64.as_bytes())
        .map_err(|_| ApiError::BadRequest("invalid_base64"))?;
    if bytes.is_empty() || bytes.len() > crate::storage::MAX_ATTACHMENT_BYTES {
        return Err(ApiError::Validation("file_size"));
    }
    if bytes.starts_with(b"%PDF-") || crate::intake::image_kind(&bytes).is_some() {
        return Err(ApiError::Validation("unsupported"));
    }
    let table = {
        let _permit = state
            .upload_permits
            .acquire()
            .await
            .map_err(ApiError::internal)?;
        crate::intake::table_rows(&file_name, &bytes)
            .await
            .map_err(crate::api::history::intake_error)?
    };
    let header =
        request_import::find_header(&table.rows).ok_or(ApiError::Validation("header_not_found"))?;
    let parsed = request_import::parse(&table.rows, header);
    if parsed.groups.is_empty() {
        return Err(ApiError::Validation("no_readable_rows"));
    }
    let stem = file_name
        .rsplit_once('.')
        .map_or(file_name.as_str(), |(s, _)| s)
        .to_string();
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let mut created = Vec::with_capacity(parsed.groups.len());
    let mut lines = 0usize;
    for g in &parsed.groups {
        let title: String = if g.name.is_empty() {
            stem.chars().take(200).collect()
        } else {
            g.name.clone()
        };
        let title = common::required(&title, 200, false, "title")?;
        let needed_by = g
            .needed_by
            .and_then(|d| NaiveDate::from_ymd_opt(d.year, d.month, d.day));
        let id = Uuid::now_v7();
        let number: String = sqlx::query_scalar("SELECT next_number($1, 'PR')")
            .bind(org_id)
            .fetch_one(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO procurement_requests (id, org_id, number, title, description, currency,
                                               needed_by, delivery_location, requested_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(id)
        .bind(org_id)
        .bind(&number)
        .bind(&title)
        .bind(format!("Imported from {file_name}"))
        .bind(currency.code())
        .bind(needed_by)
        .bind(&g.delivery_location)
        .bind(p.user_id)
        .execute(&mut *tx)
        .await?;
        for l in &g.lines {
            let item = clean_item(&ItemInput {
                description: l.description.clone(),
                manufacturer: l.manufacturer.clone(),
                mpn: l.mpn.clone(),
                quantity: l.quantity,
                unit: l.unit.clone(),
                notes: l.notes.clone(),
            })?;
            insert_item(&mut tx, org_id, id, &item).await?;
        }
        lines += g.lines.len();
        Audit { org_id, p: &p }
            .record(
                &mut tx,
                "request.created",
                "procurement_request",
                id,
                json!({ "number": number, "items": g.lines.len(), "currency": currency.code(),
                        "imported_from": file_name, "rows": g.rows }),
            )
            .await?;
        created.push(Created { id, number });
    }
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(ImportRequestsResponse {
            created,
            lines,
            skipped: parsed
                .skipped
                .iter()
                .map(|(row, e)| SkippedRow {
                    row: *row,
                    reason: e.code(),
                })
                .collect(),
        }),
    ))
}
