//! Matching (agent layer L2): per-organisation match tolerances and AI invoice extraction
//! from pasted supplier invoice text. The matching agent itself is `agents::matching`.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use procure_ai::extract::Outcome;
use procure_ai::invoice::{self, InvoiceTarget};
use procure_domain::rbac::Permission;
use procure_domain::workflow::RequestStatus;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::AppState;
use crate::agents::matching::load_tolerance;
use crate::api::ai::{
    ExtractBody, NewSuggestion, SuggestionView, ai_allowed, check_text, rate_limit, store,
};
use crate::api::common::{self, Audit, View, lock_request};
use crate::db;
use crate::error::{ApiError, ApiJson, ApiPath, ApiResult, ErrorBody};
use crate::principal::Principal;

/// Match tolerances. A zero limit is not set; a difference is within tolerance only when it is
/// within every limit set for its kind. All zero (the default) = exact: nothing is proposed for
/// acceptance.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct MatchTolerances {
    /// Currency of the absolute limits (never applied to another currency).
    pub currency: String,
    /// Unit price difference as a share of the ordered price, in basis points (100 = 1%).
    pub price_bps: u32,
    /// A line's price difference (unit difference × invoiced quantity), minor units.
    pub price_line_abs_minor: i64,
    /// Quantity difference as a share of the ordered/received quantity, basis points.
    pub quantity_bps: u32,
    /// Quantity difference in units.
    pub quantity_abs: u32,
    /// Tax difference allowed for rounding, minor units.
    pub rounding_minor: i64,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct MatchTolerancesView {
    #[serde(flatten)]
    pub tolerances: MatchTolerances,
    pub updated_by: Option<Uuid>,
    pub updated_at: Option<DateTime<Utc>>,
}

const MAX_LINE_ABS_MINOR: i64 = 1_000_000_000_000;
const MAX_ROUNDING_MINOR: i64 = 1_000_000;

async fn load_view(tx: &mut db::Tx, org_id: Uuid) -> ApiResult<MatchTolerancesView> {
    let row = load_tolerance(tx, org_id).await?;
    let meta: Option<(Option<Uuid>, DateTime<Utc>)> =
        sqlx::query_as("SELECT updated_by, updated_at FROM match_tolerances WHERE org_id = $1")
            .bind(org_id)
            .fetch_optional(&mut **tx)
            .await?;
    let u = |v: i32| u32::try_from(v).unwrap_or(0);
    Ok(MatchTolerancesView {
        tolerances: MatchTolerances {
            currency: row.currency,
            price_bps: u(row.price_bps),
            price_line_abs_minor: row.price_line_abs_minor,
            quantity_bps: u(row.quantity_bps),
            quantity_abs: u(row.quantity_abs),
            rounding_minor: row.rounding_minor,
        },
        updated_by: meta.and_then(|m| m.0),
        updated_at: meta.map(|m| m.1),
    })
}

#[utoipa::path(
    operation_id = "matching_get_tolerances",
    get,
    path = "/api/v1/orgs/{org_id}/matching/tolerances",
    tag = "matching",
    params(("org_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Match tolerances (all zero = exact when never set)", body = MatchTolerancesView),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn get_tolerances(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
) -> ApiResult<Json<MatchTolerancesView>> {
    p.require(&state, org_id, Permission::ViewOrders).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let view = load_view(&mut tx, org_id).await?;
    tx.commit().await?;
    Ok(Json(view))
}

#[utoipa::path(
    operation_id = "matching_put_tolerances",
    put,
    path = "/api/v1/orgs/{org_id}/matching/tolerances",
    tag = "matching",
    params(("org_id" = Uuid, Path)),
    request_body = MatchTolerances,
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Tolerances saved", body = MatchTolerancesView),
        (status = 403, description = "Not permitted (finance decides tolerances)", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 422, description = "Out of range", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn put_tolerances(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<MatchTolerances>,
) -> ApiResult<Json<MatchTolerancesView>> {
    p.require(&state, org_id, Permission::ManageInvoices)
        .await?;
    let currency = common::currency(&body.currency)?;
    let bps = |v: u32, f: &'static str| {
        if v <= 10_000 {
            i32::try_from(v).map_err(|_| ApiError::Validation(f))
        } else {
            Err(ApiError::Validation(f))
        }
    };
    let price_bps = bps(body.price_bps, "price_bps")?;
    let quantity_bps = bps(body.quantity_bps, "quantity_bps")?;
    if !(0..=MAX_LINE_ABS_MINOR).contains(&body.price_line_abs_minor) {
        return Err(ApiError::Validation("price_line_abs_minor"));
    }
    if !(0..=MAX_ROUNDING_MINOR).contains(&body.rounding_minor) {
        return Err(ApiError::Validation("rounding_minor"));
    }
    let quantity_abs = i32::try_from(body.quantity_abs)
        .ok()
        .filter(|q| *q <= 1_000_000_000)
        .ok_or(ApiError::Validation("quantity_abs"))?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let before = load_view(&mut tx, org_id).await?;
    sqlx::query(
        "INSERT INTO match_tolerances (org_id, currency, price_bps, price_line_abs_minor,
                                       quantity_bps, quantity_abs, rounding_minor, updated_by,
                                       updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now())
         ON CONFLICT (org_id) DO UPDATE SET currency = EXCLUDED.currency,
             price_bps = EXCLUDED.price_bps,
             price_line_abs_minor = EXCLUDED.price_line_abs_minor,
             quantity_bps = EXCLUDED.quantity_bps, quantity_abs = EXCLUDED.quantity_abs,
             rounding_minor = EXCLUDED.rounding_minor, updated_by = EXCLUDED.updated_by,
             updated_at = now()",
    )
    .bind(org_id)
    .bind(currency.code())
    .bind(price_bps)
    .bind(body.price_line_abs_minor)
    .bind(quantity_bps)
    .bind(quantity_abs)
    .bind(body.rounding_minor)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    let after = load_view(&mut tx, org_id).await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "matching.tolerances_updated",
            "organisation",
            org_id,
            json!({ "before": before.tolerances, "after": after.tolerances }),
        )
        .await?;
    tx.commit().await?;
    Ok(Json(after))
}

#[utoipa::path(
    operation_id = "matching_extract_invoice",
    post,
    path = "/api/v1/orgs/{org_id}/orders/{order_id}/ai/extract-invoice",
    tag = "matching",
    params(("org_id" = Uuid, Path), ("order_id" = Uuid, Path)),
    request_body = ExtractBody,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Suggestion stored (pending, or failed with error_code); output lines carry the order_item_id they bill", body = SuggestionView),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or not visible order", body = ErrorBody),
        (status = 409, description = "Not delivered yet, already invoiced, or AI switched off", body = ErrorBody),
        (status = 422, description = "Empty or too long text", body = ErrorBody),
        (status = 429, description = "AI rate limit reached", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn extract_invoice(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, order_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<ExtractBody>,
) -> ApiResult<(StatusCode, Json<SuggestionView>)> {
    p.require(&state, org_id, Permission::ManageInvoices)
        .await?;
    let text = check_text(&body.text)?;
    let (targets, ids, currency) = {
        let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
        let (request_id, currency): (Uuid, String) =
            sqlx::query_as("SELECT request_id, currency FROM purchase_orders WHERE id = $1")
                .bind(order_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(ApiError::NotFound)?;
        let req = lock_request(&mut tx, request_id, View::All).await?;
        if req.status()? != RequestStatus::Delivered {
            return Err(ApiError::Conflict("invalid_transition"));
        }
        let rows: Vec<(Uuid, i32, String, Option<String>, i32, i64)> = sqlx::query_as(
            "SELECT oi.id, ri.line_no, ri.description, coalesce(ri.verified_mpn, ri.mpn),
                    oi.quantity, oi.unit_price_minor
             FROM order_items oi JOIN request_items ri ON ri.id = oi.request_item_id
             WHERE oi.order_id = $1 ORDER BY ri.line_no",
        )
        .bind(order_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let mut targets = Vec::with_capacity(rows.len());
        let mut ids = Vec::with_capacity(rows.len());
        for (id, line_no, description, mpn, quantity, price) in rows {
            let (Ok(line_no), Ok(quantity)) = (u32::try_from(line_no), u32::try_from(quantity))
            else {
                continue;
            };
            ids.push((line_no, id));
            targets.push(InvoiceTarget {
                line_no,
                description,
                mpn: mpn.unwrap_or_default(),
                quantity,
                unit_price_minor: price,
            });
        }
        (targets, ids, common::currency(&currency)?)
    };
    ai_allowed(&state, &p, org_id).await?;
    rate_limit(&state, &p)?;
    let result = {
        let _permit = state
            .ai_permits
            .acquire()
            .await
            .map_err(ApiError::internal)?;
        invoice::extract_invoice(state.ai.as_ref(), &text, &targets, currency).await
    };
    // Each suggested line also names the order item it bills, ready for the invoice form.
    let result = result.and_then(|o| {
        let mut value =
            serde_json::to_value(&o.value).map_err(|_| procure_ai::AiError::InvalidOutput)?;
        if let Some(lines) = value.get_mut("lines").and_then(Value::as_array_mut) {
            for l in lines {
                let item = l
                    .get("line_no")
                    .and_then(Value::as_u64)
                    .and_then(|n| ids.iter().find(|(no, _)| u64::from(*no) == n))
                    .map(|(_, id)| *id);
                l["order_item_id"] = json!(item);
            }
        }
        Ok(Outcome {
            value,
            provenance: o.provenance,
        })
    });
    let stored = store(
        &state,
        &p,
        NewSuggestion {
            org_id,
            capability: invoice::EXTRACT_INVOICE,
            subject_type: "purchase_order",
            subject_id: order_id,
            supplier_id: None,
            text: &text,
            source: "typed",
        },
        &result,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(stored)))
}
