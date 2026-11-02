//! Orders and finance: the customer records its own purchase order for the approved
//! recommendation, confirms deliveries, and records the supplier's invoice, which is matched
//! three ways (procure_domain::matching). Differences become discrepancies a person resolves.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, NaiveDate, Utc};
use procure_domain::matching::{self, Discrepancy, Invoice, InvoiceLine, OrderLine, TaxRounding};
use procure_domain::money::Money;
use procure_domain::rbac::{self, Permission};
use procure_domain::workflow::{RequestStatus, Side};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::api::common::{
    self, Audit, View, lock_request, request_view, require_perm, side, transition,
};
use crate::api::history_orders;
use crate::api::savings;
use crate::api::suppliers::IdResponse;
use crate::db;
use crate::error::{ApiError, ApiJson, ApiPath, ApiResult, ErrorBody};
use crate::principal::Principal;

const RESOLUTIONS: [&str; 5] = [
    "accepted",
    "credit_note",
    "supplier_corrected",
    "written_off",
    "other",
];

fn to_u32(v: i32) -> ApiResult<u32> {
    u32::try_from(v).map_err(|_| ApiError::internal(std::io::Error::other("negative quantity")))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct RecordOrderBody {
    /// The customer's own purchase order number.
    pub po_number: String,
    #[serde(default)]
    pub notes: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/order",
    tag = "orders",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    request_body = RecordOrderBody,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Purchase order recorded from the approved recommendation", body = IdResponse),
        (status = 403, description = "Not permitted (customer decision)", body = ErrorBody),
        (status = 404, description = "Unknown or not visible", body = ErrorBody),
        (status = 409, description = "Not approved, or PO number already used", body = ErrorBody),
        (status = 422, description = "Invalid PO number", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn record_order(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<RecordOrderBody>,
) -> ApiResult<(StatusCode, Json<IdResponse>)> {
    let access = p.require(&state, org_id, Permission::RecordOrders).await?;
    let side = side(access)?;
    let po_number = common::required(&body.po_number, 100, false, "po_number")?;
    let notes = common::text(&body.notes, 2000, true, "notes")?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let req = lock_request(&mut tx, request_id, View::All).await?;
    if req.status()? != RequestStatus::Approved {
        return Err(ApiError::Conflict("invalid_transition"));
    }
    // The approved recommendation and its quotation are what is ordered: nothing else.
    let (rec_id, quotation_id, total): (Uuid, Uuid, i64) = sqlx::query_as(
        "SELECT id, quotation_id, total_minor FROM recommendations
         WHERE request_id = $1 AND status = 'approved' ORDER BY decided_at DESC LIMIT 1",
    )
    .bind(req.id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::Conflict("no_approved_recommendation"))?;
    let (supplier_id, currency, shipping): (Uuid, String, i64) = sqlx::query_as(
        "SELECT x.supplier_id, q.currency, q.shipping_minor FROM quotations q
         JOIN rfq_recipients x ON x.id = q.rfq_recipient_id WHERE q.id = $1",
    )
    .bind(quotation_id)
    .fetch_one(&mut *tx)
    .await?;
    // The order must be exactly what was approved: recompute from the quotation lines.
    let (line_count, lines_total): (i64, Option<i64>) = sqlx::query_as(
        "SELECT count(*), sum(unit_price_minor * quantity)::bigint FROM quotation_lines
         WHERE quotation_id = $1",
    )
    .bind(quotation_id)
    .fetch_one(&mut *tx)
    .await?;
    if line_count == 0 || lines_total.and_then(|t| t.checked_add(shipping)) != Some(total) {
        return Err(ApiError::Conflict("recommendation_inconsistent"));
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO purchase_orders (id, org_id, request_id, recommendation_id, supplier_id, po_number,
                                      currency, shipping_minor, total_minor, notes, ordered_by,
                                      promised_on)
         SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                -- The slowest quoted line: the day everything should be here.
                (now() AT TIME ZONE g.timezone)::date
                    + (SELECT max(l.lead_time_days) FROM quotation_lines l WHERE l.quotation_id = $12)
         FROM organisations g WHERE g.id = $2",
    )
    .bind(id)
    .bind(org_id)
    .bind(req.id)
    .bind(rec_id)
    .bind(supplier_id)
    .bind(&po_number)
    .bind(&currency)
    .bind(shipping)
    .bind(total)
    .bind(&notes)
    .bind(p.user_id)
    .bind(quotation_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO order_items (id, org_id, order_id, request_item_id, quantity, unit_price_minor,
                                  offered_mpn, alternative)
         SELECT gen_random_uuid(), $1, $2, l.request_item_id, l.quantity, l.unit_price_minor,
                l.offered_mpn, l.alternative
         FROM quotation_lines l WHERE l.quotation_id = $3",
    )
    .bind(org_id)
    .bind(id)
    .bind(quotation_id)
    .execute(&mut *tx)
    .await?;
    transition(
        &mut tx,
        &Audit { org_id, p: &p },
        &req,
        RequestStatus::Ordered,
        side,
        "order.recorded",
        json!({ "order": id, "po_number": po_number, "supplier": supplier_id,
                "total_minor": total, "currency": currency }),
    )
    .await?;
    open_delivery_check(&mut tx, org_id, req.id, id).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(IdResponse { id })))
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct OrderHeader {
    pub id: Uuid,
    pub request_id: Uuid,
    pub po_number: String,
    pub supplier_id: Uuid,
    pub supplier_name: Option<String>,
    pub currency: String,
    pub shipping_minor: i64,
    pub total_minor: i64,
    pub notes: String,
    pub ordered_at: DateTime<Utc>,
    /// From the quotation's longest lead time (null when the supplier gave none).
    pub promised_on: Option<NaiveDate>,
    /// The date the supplier confirmed to our staff.
    pub delivery_confirmed_on: Option<NaiveDate>,
    pub delivery_note: String,
    /// Confirmed date, else promised date.
    pub expected_on: Option<NaiveDate>,
    /// Not everything has arrived and the expected day has passed (organisation's calendar).
    pub late: bool,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct OrderItemView {
    pub id: Uuid,
    pub line_no: i32,
    pub description: String,
    pub quantity: i32,
    pub unit: String,
    pub unit_price_minor: i64,
    pub offered_mpn: String,
    pub alternative: bool,
    pub received: i64,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct DeliveryView {
    pub id: Uuid,
    pub received_at: DateTime<Utc>,
    pub note: String,
    pub received_by_name: Option<String>,
    pub reversed_at: Option<DateTime<Utc>>,
    pub reversal_reason: Option<String>,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct InvoiceView {
    pub id: Uuid,
    pub supplier_invoice_number: String,
    pub invoice_date: NaiveDate,
    pub due_date: Option<NaiveDate>,
    pub currency: String,
    pub shipping_minor: i64,
    pub tax_rate_bps: i32,
    pub tax_rounding: String,
    pub tax_minor: i64,
    pub subtotal_minor: i64,
    pub total_minor: i64,
    /// `matched`, `disputed` or `reconciled`.
    pub match_status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct DiscrepancyView {
    pub id: Uuid,
    pub kind: String,
    pub order_item_id: Option<Uuid>,
    pub line_no: Option<i32>,
    pub expected: Option<i64>,
    pub actual: Option<i64>,
    pub status: String,
    pub resolution: Option<String>,
    pub resolution_note: String,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct OrderDetail {
    pub order: OrderHeader,
    pub items: Vec<OrderItemView>,
    pub deliveries: Vec<DeliveryView>,
    pub invoice: Option<InvoiceView>,
    pub discrepancies: Vec<DiscrepancyView>,
}

/// Who may read orders: view_orders / finance roles, receivers of their own requests, and
/// assigned staff with view_orders.
async fn order_reader(p: &Principal, state: &AppState, org_id: Uuid) -> ApiResult<View> {
    let (access, view) = request_view(p, state, org_id).await?;
    let ok = [
        Permission::ViewOrders,
        Permission::ViewFinance,
        Permission::ManageInvoices,
        Permission::ReceiveGoods,
    ]
    .into_iter()
    .any(|perm| rbac::allows(access, perm));
    if ok {
        Ok(view)
    } else {
        Err(ApiError::Forbidden)
    }
}

async fn load_detail(tx: &mut db::Tx, request_id: Uuid) -> ApiResult<OrderDetail> {
    let order = sqlx::query_as::<_, OrderHeader>(
        "SELECT o.id, o.request_id, o.po_number, o.supplier_id, s.name AS supplier_name, o.currency,
                o.shipping_minor, o.total_minor, o.notes, o.ordered_at, o.promised_on,
                o.delivery_confirmed_on, o.delivery_note,
                coalesce(o.delivery_confirmed_on, o.promised_on) AS expected_on,
                (r.status = 'ordered'
                 AND coalesce(o.delivery_confirmed_on, o.promised_on)
                     < (now() AT TIME ZONE g.timezone)::date) IS TRUE AS late
         FROM purchase_orders o
         JOIN procurement_requests r ON r.id = o.request_id
         JOIN organisations g ON g.id = o.org_id
         LEFT JOIN suppliers s ON s.id = o.supplier_id
         WHERE o.request_id = $1",
    )
    .bind(request_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    let items = sqlx::query_as::<_, OrderItemView>(
        "SELECT oi.id, i.line_no, i.description, oi.quantity, i.unit, oi.unit_price_minor,
                oi.offered_mpn, oi.alternative,
                coalesce((SELECT sum(d.quantity) FROM delivery_lines d JOIN deliveries dv ON dv.id = d.delivery_id
                          WHERE d.order_item_id = oi.id AND dv.reversed_at IS NULL), 0)::bigint
                    AS received
         FROM order_items oi JOIN request_items i ON i.id = oi.request_item_id
         WHERE oi.order_id = $1 ORDER BY i.line_no",
    )
    .bind(order.id)
    .fetch_all(&mut **tx)
    .await?;
    let deliveries = sqlx::query_as::<_, DeliveryView>(
        "SELECT d.id, d.received_at, d.note, u.display_name AS received_by_name, d.reversed_at,
                d.reversal_reason
         FROM deliveries d LEFT JOIN users u ON u.id = d.received_by
         WHERE d.order_id = $1 ORDER BY d.received_at, d.id",
    )
    .bind(order.id)
    .fetch_all(&mut **tx)
    .await?;
    let invoice = sqlx::query_as::<_, InvoiceView>(
        "SELECT id, supplier_invoice_number, invoice_date, due_date, currency, shipping_minor,
                tax_rate_bps, tax_rounding, tax_minor, subtotal_minor, total_minor, match_status,
                created_at
         FROM invoices WHERE order_id = $1",
    )
    .bind(order.id)
    .fetch_optional(&mut **tx)
    .await?;
    let discrepancies =
        match &invoice {
            Some(inv) => sqlx::query_as::<_, DiscrepancyView>(
                "SELECT d.id, d.kind, d.order_item_id, i.line_no, d.expected, d.actual, d.status,
                        d.resolution, d.resolution_note, d.resolved_at
                 FROM discrepancies d
                 LEFT JOIN order_items oi ON oi.id = d.order_item_id
                 LEFT JOIN request_items i ON i.id = oi.request_item_id
                 WHERE d.invoice_id = $1 ORDER BY d.status DESC, i.line_no NULLS LAST, d.id",
            )
            .bind(inv.id)
            .fetch_all(&mut **tx)
            .await?,
            None => Vec::new(),
        };
    Ok(OrderDetail {
        order,
        items,
        deliveries,
        invoice,
        discrepancies,
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/order",
    tag = "orders",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Order with items, deliveries, invoice and discrepancies", body = OrderDetail),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "No order, or not visible", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn get_order(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<OrderDetail>> {
    let view = order_reader(&p, &state, org_id).await?;
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
    let detail = load_detail(&mut tx, request_id).await?;
    tx.commit().await?;
    Ok(Json(detail))
}

async fn order_request(tx: &mut db::Tx, order_id: Uuid) -> ApiResult<Uuid> {
    sqlx::query_scalar("SELECT request_id FROM purchase_orders WHERE id = $1")
        .bind(order_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct DeliveryLineInput {
    pub order_item_id: Uuid,
    pub quantity: u32,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct DeliveryInput {
    pub received_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub note: String,
    pub lines: Vec<DeliveryLineInput>,
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/orders/{order_id}/deliveries",
    tag = "orders",
    params(("org_id" = Uuid, Path), ("order_id" = Uuid, Path)),
    request_body = DeliveryInput,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Receipt recorded; the request becomes delivered once everything has arrived", body = IdResponse),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or not visible", body = ErrorBody),
        (status = 409, description = "Order is not open for receiving", body = ErrorBody),
        (status = 422, description = "Unknown line or more than ordered", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn record_delivery(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, order_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<DeliveryInput>,
) -> ApiResult<(StatusCode, Json<IdResponse>)> {
    let (access, view) = request_view(&p, &state, org_id).await?;
    require_perm(access, Permission::ReceiveGoods)?;
    let side = side(access)?;
    let note = common::text(&body.note, 2000, true, "note")?;
    if body.lines.is_empty()
        || body
            .lines
            .iter()
            .any(|l| l.quantity == 0 || l.quantity > 1_000_000_000)
    {
        return Err(ApiError::Validation("lines"));
    }
    let received_at = body.received_at.unwrap_or_else(Utc::now);
    if received_at > Utc::now() + chrono::Duration::minutes(5) {
        return Err(ApiError::Validation("received_at"));
    }
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let request_id = order_request(&mut tx, order_id).await?;
    let req = lock_request(&mut tx, request_id, view).await?;
    if req.status()? != RequestStatus::Ordered {
        return Err(ApiError::Conflict("not_receiving"));
    }
    let items: Vec<(Uuid, i32, i64)> = sqlx::query_as(
        "SELECT oi.id, oi.quantity,
                coalesce((SELECT sum(d.quantity) FROM delivery_lines d JOIN deliveries dv ON dv.id = d.delivery_id
                          WHERE d.order_item_id = oi.id AND dv.reversed_at IS NULL), 0)::bigint
         FROM order_items oi WHERE oi.order_id = $1",
    )
    .bind(order_id)
    .fetch_all(&mut *tx)
    .await?;
    for (i, l) in body.lines.iter().enumerate() {
        if body.lines[..i]
            .iter()
            .any(|o| o.order_item_id == l.order_item_id)
        {
            return Err(ApiError::Validation("duplicate_line"));
        }
        let (_, ordered, received) = items
            .iter()
            .find(|(id, _, _)| *id == l.order_item_id)
            .ok_or(ApiError::Validation("order_item_id"))?;
        if received + i64::from(l.quantity) > i64::from(*ordered) {
            // More than ordered: a discrepancy with the supplier, not something to book.
            return Err(ApiError::Validation("over_received"));
        }
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO deliveries (id, org_id, order_id, received_at, note, received_by)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(org_id)
    .bind(order_id)
    .bind(received_at)
    .bind(&note)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    for l in &body.lines {
        sqlx::query(
            "INSERT INTO delivery_lines (id, org_id, delivery_id, order_item_id, quantity)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(Uuid::now_v7())
        .bind(org_id)
        .bind(id)
        .bind(l.order_item_id)
        .bind(i32::try_from(l.quantity).map_err(|_| ApiError::Validation("lines"))?)
        .execute(&mut *tx)
        .await?;
    }
    let audit = Audit { org_id, p: &p };
    audit
        .record(
            &mut tx,
            "delivery.recorded",
            "delivery",
            id,
            json!({ "order": order_id, "lines": body.lines.len() }),
        )
        .await?;
    let outstanding: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM order_items oi
         WHERE oi.order_id = $1 AND oi.quantity >
               coalesce((SELECT sum(d.quantity) FROM delivery_lines d JOIN deliveries dv ON dv.id = d.delivery_id
                          WHERE d.order_item_id = oi.id AND dv.reversed_at IS NULL), 0)",
    )
    .bind(order_id)
    .fetch_one(&mut *tx)
    .await?;
    if outstanding == 0 {
        transition(
            &mut tx,
            &audit,
            &req,
            RequestStatus::Delivered,
            side,
            "request.delivered",
            json!({ "order": order_id }),
        )
        .await?;
        crate::api::ops::close_auto_tasks(&mut tx, req.id, None, "check_delivery").await?;
    }
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(IdResponse { id })))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct InvoiceLineInput {
    /// Omit for something the supplier billed that is not on the order (then `description`
    /// is required); it becomes a discrepancy.
    pub order_item_id: Option<Uuid>,
    #[serde(default)]
    pub description: String,
    pub quantity: u32,
    pub unit_price_minor: i64,
    /// Defaults to the invoice's rate.
    pub tax_rate_bps: Option<u32>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct InvoiceInput {
    pub supplier_invoice_number: String,
    pub invoice_date: NaiveDate,
    pub due_date: Option<NaiveDate>,
    pub currency: String,
    #[serde(default)]
    pub shipping_minor: i64,
    /// Tax rate in basis points (1000 = 10%).
    pub tax_rate_bps: u32,
    /// `floor` (default), `half_up` or `ceil`.
    #[serde(default = "floor")]
    pub tax_rounding: String,
    /// Tax as printed on the invoice.
    pub tax_minor: i64,
    pub lines: Vec<InvoiceLineInput>,
}

fn floor() -> String {
    "floor".into()
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct InvoiceResult {
    pub id: Uuid,
    /// `matched` (request reconciled) or `disputed` (discrepancies to resolve).
    pub match_status: String,
    pub discrepancies: usize,
}

fn discrepancy_values(d: &Discrepancy<Uuid>) -> (Option<Uuid>, Option<i64>, Option<i64>) {
    match d {
        Discrepancy::UnknownItem { item }
        | Discrepancy::DuplicateLine { item }
        | Discrepancy::NotInvoiced { item } => (Some(*item), None, None),
        Discrepancy::PriceMismatch {
            item,
            ordered,
            invoiced,
        } => (Some(*item), Some(ordered.minor()), Some(invoiced.minor())),
        Discrepancy::QuantityOverOrdered {
            item,
            ordered,
            invoiced,
        } => (
            Some(*item),
            Some(i64::from(*ordered)),
            Some(i64::from(*invoiced)),
        ),
        Discrepancy::QuantityOverReceived {
            item,
            received,
            invoiced,
        }
        | Discrepancy::QuantityUnderReceived {
            item,
            received,
            invoiced,
        } => (
            Some(*item),
            Some(i64::from(*received)),
            Some(i64::from(*invoiced)),
        ),
        Discrepancy::ShippingMismatch { ordered, invoiced } => {
            (None, Some(ordered.minor()), Some(invoiced.minor()))
        }
        Discrepancy::TaxMismatch { expected, invoiced } => {
            (None, Some(expected.minor()), Some(invoiced.minor()))
        }
        Discrepancy::CurrencyMismatch | Discrepancy::Overflow => (None, None, None),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/orders/{order_id}/invoice",
    tag = "orders",
    params(("org_id" = Uuid, Path), ("order_id" = Uuid, Path)),
    request_body = InvoiceInput,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Invoice recorded and matched", body = InvoiceResult),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or not visible", body = ErrorBody),
        (status = 409, description = "Not delivered yet, or already invoiced", body = ErrorBody),
        (status = 422, description = "Invalid invoice", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn record_invoice(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, order_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<InvoiceInput>,
) -> ApiResult<(StatusCode, Json<InvoiceResult>)> {
    let access = p
        .require(&state, org_id, Permission::ManageInvoices)
        .await?;
    let side = side(access)?;
    let number = common::required(
        &body.supplier_invoice_number,
        100,
        false,
        "supplier_invoice_number",
    )?;
    let currency = common::currency(&body.currency)?;
    let rounding =
        TaxRounding::from_code(&body.tax_rounding).ok_or(ApiError::Validation("tax_rounding"))?;
    if body.tax_rate_bps > 10_000 {
        return Err(ApiError::Validation("tax_rate_bps"));
    }
    if body.shipping_minor < 0 || body.tax_minor < 0 {
        return Err(ApiError::Validation("amount"));
    }
    if body.lines.is_empty()
        || body.lines.len() > crate::api::requests::MAX_ITEMS
        || body
            .lines
            .iter()
            .any(|l| l.unit_price_minor < 0 || l.quantity == 0 || l.quantity > 1_000_000_000)
    {
        return Err(ApiError::Validation("lines"));
    }
    if body.due_date.is_some_and(|d| d < body.invoice_date) {
        return Err(ApiError::Validation("due_date"));
    }
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let request_id = order_request(&mut tx, order_id).await?;
    let req = lock_request(&mut tx, request_id, View::All).await?;
    if req.status()? != RequestStatus::Delivered {
        return Err(ApiError::Conflict("invalid_transition"));
    }
    let (order_currency, order_shipping): (String, i64) =
        sqlx::query_as("SELECT currency, shipping_minor FROM purchase_orders WHERE id = $1")
            .bind(order_id)
            .fetch_one(&mut *tx)
            .await?;
    let order_currency = common::currency(&order_currency)?;
    let rows: Vec<(Uuid, i32, i64, i64)> = sqlx::query_as(
        "SELECT oi.id, oi.quantity, oi.unit_price_minor,
                coalesce((SELECT sum(d.quantity) FROM delivery_lines d JOIN deliveries dv ON dv.id = d.delivery_id
                          WHERE d.order_item_id = oi.id AND dv.reversed_at IS NULL), 0)::bigint
         FROM order_items oi WHERE oi.order_id = $1",
    )
    .bind(order_id)
    .fetch_all(&mut *tx)
    .await?;
    if body.lines.iter().any(|l| {
        l.order_item_id
            .is_some_and(|i| !rows.iter().any(|(id, ..)| *id == i))
    }) {
        return Err(ApiError::Validation("order_item_id"));
    }
    if body.lines.iter().any(|l| {
        l.tax_rate_bps.is_some_and(|r| r > 10_000)
            || (l.order_item_id.is_none() && l.description.trim().is_empty())
            || l.description.chars().count() > 500
    }) {
        return Err(ApiError::Validation("lines"));
    }
    // Lines that are not on the order get their own key: the match reports them.
    let keys: Vec<Uuid> = body
        .lines
        .iter()
        .map(|l| l.order_item_id.unwrap_or_else(Uuid::now_v7))
        .collect();
    let supplier_id: Uuid =
        sqlx::query_scalar("SELECT supplier_id FROM purchase_orders WHERE id = $1")
            .bind(order_id)
            .fetch_one(&mut *tx)
            .await?;
    let order_lines = rows
        .iter()
        .map(|(id, q, price, _)| {
            Ok(OrderLine {
                item: *id,
                quantity: to_u32(*q)?,
                unit_price: Money::from_minor(*price, order_currency),
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let received: Vec<(Uuid, u32)> = rows
        .iter()
        .map(|(id, _, _, r)| (*id, u32::try_from(*r).unwrap_or(u32::MAX)))
        .collect();
    let invoice = Invoice {
        currency,
        lines: body
            .lines
            .iter()
            .zip(&keys)
            .map(|(l, key)| InvoiceLine {
                item: *key,
                quantity: l.quantity,
                unit_price: Money::from_minor(l.unit_price_minor, currency),
                tax_rate_bps: l.tax_rate_bps.unwrap_or(body.tax_rate_bps),
            })
            .collect(),
        shipping: Money::from_minor(body.shipping_minor, currency),
        shipping_tax_rate_bps: body.tax_rate_bps,
        tax_rounding: rounding,
        tax: Money::from_minor(body.tax_minor, currency),
    };
    let found = matching::three_way(
        &order_lines,
        order_currency,
        Money::from_minor(order_shipping, order_currency),
        &received,
        &invoice,
    );
    let totals = matching::totals(&invoice).map_err(|_| ApiError::Validation("amount_overflow"))?;
    let status = if found.is_empty() {
        "matched"
    } else {
        "disputed"
    };
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO invoices (id, org_id, order_id, supplier_invoice_number, invoice_date, due_date,
                               currency, shipping_minor, tax_rate_bps, tax_rounding, tax_minor,
                               subtotal_minor, total_minor, match_status, recorded_by, supplier_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
    )
    .bind(id)
    .bind(org_id)
    .bind(order_id)
    .bind(&number)
    .bind(body.invoice_date)
    .bind(body.due_date)
    .bind(currency.code())
    .bind(body.shipping_minor)
    .bind(i32::try_from(body.tax_rate_bps).map_err(|_| ApiError::Validation("tax_rate_bps"))?)
    .bind(&body.tax_rounding)
    .bind(body.tax_minor)
    .bind(totals.subtotal.minor())
    .bind(totals.total.minor())
    .bind(status)
    .bind(p.user_id)
    .bind(supplier_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| match ApiError::from(e) {
        ApiError::Conflict(_) => ApiError::Conflict("invoice_already_recorded"),
        other => other,
    })?;
    for l in &body.lines {
        sqlx::query(
            "INSERT INTO invoice_lines (id, org_id, invoice_id, order_item_id, description, quantity,
                                        unit_price_minor, tax_rate_bps)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(Uuid::now_v7())
        .bind(org_id)
        .bind(id)
        .bind(l.order_item_id)
        .bind(l.description.trim())
        .bind(i32::try_from(l.quantity).map_err(|_| ApiError::Validation("lines"))?)
        .bind(l.unit_price_minor)
        .bind(
            i32::try_from(l.tax_rate_bps.unwrap_or(body.tax_rate_bps))
                .map_err(|_| ApiError::Validation("lines"))?,
        )
        .execute(&mut *tx)
        .await?;
    }
    for d in &found {
        let (item, expected, actual) = discrepancy_values(d);
        // Only real order items are referenced; lines not on the order have none.
        let item = item.filter(|i| rows.iter().any(|(id, ..)| id == i));
        sqlx::query(
            "INSERT INTO discrepancies (id, org_id, invoice_id, kind, order_item_id, expected, actual)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(Uuid::now_v7())
        .bind(org_id)
        .bind(id)
        .bind(d.code())
        .bind(item)
        .bind(expected)
        .bind(actual)
        .execute(&mut *tx)
        .await?;
    }
    let audit = Audit { org_id, p: &p };
    audit
        .record(
            &mut tx,
            "invoice.recorded",
            "invoice",
            id,
            json!({ "order": order_id, "number": number, "total_minor": totals.total.minor(),
                    "currency": currency.code(), "match": status,
                    "discrepancies": found.iter().map(Discrepancy::code).collect::<Vec<_>>() }),
        )
        .await?;
    transition(
        &mut tx,
        &audit,
        &req,
        RequestStatus::Invoiced,
        side,
        "request.invoiced",
        json!({ "invoice": id }),
    )
    .await?;
    // `transition` updated the row; refresh the status before the next step.
    let req = lock_request(&mut tx, request_id, View::All).await?;
    let next = if found.is_empty() {
        RequestStatus::Reconciled
    } else {
        RequestStatus::Disputed
    };
    let action = if found.is_empty() {
        "request.reconciled"
    } else {
        "request.disputed"
    };
    transition(
        &mut tx,
        &audit,
        &req,
        next,
        side,
        action,
        json!({ "invoice": id, "discrepancies": found.len() }),
    )
    .await?;
    if found.is_empty() {
        savings::record_order(&mut tx, &audit, org_id, order_id, p.user_id).await?;
        history_orders::record_order(&mut tx, &audit, org_id, order_id, p.user_id).await?;
    }
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(InvoiceResult {
            id,
            match_status: status.to_string(),
            discrepancies: found.len(),
        }),
    ))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ResolveBody {
    /// `accepted`, `credit_note`, `supplier_corrected`, `written_off` or `other`.
    pub resolution: String,
    pub note: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/discrepancies/{discrepancy_id}/resolve",
    tag = "orders",
    params(("org_id" = Uuid, Path), ("discrepancy_id" = Uuid, Path)),
    request_body = ResolveBody,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Resolved; the request is reconciled when none remain open"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown discrepancy", body = ErrorBody),
        (status = 409, description = "Already resolved", body = ErrorBody),
        (status = 422, description = "Unknown resolution or missing note", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn resolve_discrepancy(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<ResolveBody>,
) -> ApiResult<StatusCode> {
    let access = p
        .require(&state, org_id, Permission::ManageInvoices)
        .await?;
    let side = side(access)?;
    if !RESOLUTIONS.contains(&body.resolution.as_str()) {
        return Err(ApiError::Validation("resolution"));
    }
    let note = common::required(&body.note, 2000, true, "note")?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let (invoice_id, request_id, order_id): (Uuid, Uuid, Uuid) = sqlx::query_as(
        "SELECT d.invoice_id, o.request_id, o.id FROM discrepancies d
         JOIN invoices i ON i.id = d.invoice_id JOIN purchase_orders o ON o.id = i.order_id
         WHERE d.id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    let req = lock_request(&mut tx, request_id, View::All).await?;
    let updated = sqlx::query(
        "UPDATE discrepancies SET status = 'resolved', resolution = $2, resolution_note = $3,
                resolved_by = $4, resolved_at = now()
         WHERE id = $1 AND status = 'open'",
    )
    .bind(id)
    .bind(&body.resolution)
    .bind(&note)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::Conflict("already_resolved"));
    }
    let audit = Audit { org_id, p: &p };
    audit
        .record(
            &mut tx,
            "discrepancy.resolved",
            "discrepancy",
            id,
            json!({ "invoice": invoice_id, "resolution": body.resolution }),
        )
        .await?;
    let open: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM discrepancies WHERE invoice_id = $1 AND status = 'open'",
    )
    .bind(invoice_id)
    .fetch_one(&mut *tx)
    .await?;
    if open == 0 {
        sqlx::query("UPDATE invoices SET match_status = 'reconciled' WHERE id = $1")
            .bind(invoice_id)
            .execute(&mut *tx)
            .await?;
        transition(
            &mut tx,
            &audit,
            &req,
            RequestStatus::Reconciled,
            side,
            "request.reconciled",
            json!({ "invoice": invoice_id }),
        )
        .await?;
        savings::record_order(&mut tx, &audit, org_id, order_id, p.user_id).await?;
        history_orders::record_order(&mut tx, &audit, org_id, order_id, p.user_id).await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/close",
    tag = "orders",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Closed"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown request", body = ErrorBody),
        (status = 409, description = "Not reconciled", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn close_request(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    let access = p
        .require(&state, org_id, Permission::ManageInvoices)
        .await?;
    let side = side(access)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let req = lock_request(&mut tx, request_id, View::All).await?;
    transition(
        &mut tx,
        &Audit { org_id, p: &p },
        &req,
        RequestStatus::Closed,
        side,
        "request.closed",
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ReverseBody {
    pub reason: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/deliveries/{delivery_id}/reverse",
    tag = "orders",
    params(("org_id" = Uuid, Path), ("delivery_id" = Uuid, Path)),
    request_body = ReverseBody,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Receipt reversed (kept on record, no longer counted)"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or not visible", body = ErrorBody),
        (status = 409, description = "Already reversed, or already invoiced", body = ErrorBody),
        (status = 422, description = "Missing reason", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn reverse_delivery(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, delivery_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<ReverseBody>,
) -> ApiResult<StatusCode> {
    let (access, view) = request_view(&p, &state, org_id).await?;
    require_perm(access, Permission::ReceiveGoods)?;
    let side = side(access)?;
    let reason = common::required(&body.reason, 2000, true, "reason")?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let order_id: Uuid = sqlx::query_scalar("SELECT order_id FROM deliveries WHERE id = $1")
        .bind(delivery_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    let request_id = order_request(&mut tx, order_id).await?;
    let req = lock_request(&mut tx, request_id, view).await?;
    let status = req.status()?;
    if !matches!(status, RequestStatus::Ordered | RequestStatus::Delivered) {
        // Once invoiced, receipts are part of the match: correct through discrepancies.
        return Err(ApiError::Conflict("already_invoiced"));
    }
    let updated = sqlx::query(
        "UPDATE deliveries SET reversed_at = now(), reversed_by = $2, reversal_reason = $3
         WHERE id = $1 AND reversed_at IS NULL",
    )
    .bind(delivery_id)
    .bind(p.user_id)
    .bind(&reason)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::Conflict("already_reversed"));
    }
    let audit = Audit { org_id, p: &p };
    audit
        .record(
            &mut tx,
            "delivery.reversed",
            "delivery",
            delivery_id,
            json!({ "order": order_id, "reason": reason }),
        )
        .await?;
    if status == RequestStatus::Delivered {
        transition(
            &mut tx,
            &audit,
            &req,
            RequestStatus::Ordered,
            side,
            "request.receipt_reversed",
            json!({ "delivery": delivery_id }),
        )
        .await?;
        open_delivery_check(&mut tx, org_id, req.id, order_id).await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------------------
// Delivery date follow-up.

/// Days to get a date from the supplier when the quotation gave no lead time.
const UNKNOWN_LEAD_FOLLOW_UP_DAYS: i64 = 3;

/// Opens (or re-dates) the automatic "check_delivery" task: due at the end of the expected
/// day, or in a few days to ask the supplier for a date when none is known.
async fn open_delivery_check(
    tx: &mut db::Tx,
    org_id: Uuid,
    request_id: Uuid,
    order_id: Uuid,
) -> ApiResult<()> {
    let (expected, po): (Option<NaiveDate>, String) = sqlx::query_as(
        "SELECT coalesce(delivery_confirmed_on, promised_on), po_number
         FROM purchase_orders WHERE id = $1",
    )
    .bind(order_id)
    .fetch_one(&mut **tx)
    .await?;
    let due = match expected {
        Some(d) => crate::api::ops::end_of_local_day(tx, org_id, d).await?,
        None => Utc::now() + chrono::Duration::days(UNKNOWN_LEAD_FOLLOW_UP_DAYS),
    };
    crate::api::ops::close_auto_tasks(tx, request_id, None, "check_delivery").await?;
    crate::api::ops::open_auto_task(
        tx,
        org_id,
        request_id,
        None,
        "check_delivery",
        &format!("Check delivery of PO {po}"),
        due,
    )
    .await
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct DeliveryDateBody {
    /// The delivery date the supplier confirmed.
    pub confirmed_on: NaiveDate,
    #[serde(default)]
    pub note: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/orders/{order_id}/delivery-date",
    tag = "orders",
    params(("org_id" = Uuid, Path), ("order_id" = Uuid, Path)),
    request_body = DeliveryDateBody,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Confirmed date recorded; the delivery check is due that day"),
        (status = 409, description = "Already delivered or closed", body = ErrorBody),
        (status = 403, description = "Only our assigned staff record it", body = ErrorBody),
        (status = 404, description = "Unknown order", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn confirm_delivery_date(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, order_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<DeliveryDateBody>,
) -> ApiResult<StatusCode> {
    let access = p
        .require(&state, org_id, Permission::OperateRequests)
        .await?;
    if side(access)? != Side::Operator {
        return Err(ApiError::Forbidden);
    }
    let note = common::text(&body.note, 2000, true, "note")?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let request_id = order_request(&mut tx, order_id).await?;
    let req = lock_request(&mut tx, request_id, View::All).await?;
    if req.status()? != RequestStatus::Ordered {
        return Err(ApiError::Conflict("invalid_transition"));
    }
    sqlx::query(
        "UPDATE purchase_orders SET delivery_confirmed_on = $2, delivery_note = $3,
                delivery_confirmed_by = $4, delivery_confirmed_at = now()
         WHERE id = $1",
    )
    .bind(order_id)
    .bind(body.confirmed_on)
    .bind(&note)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "order.delivery_date_confirmed",
            "purchase_order",
            order_id,
            json!({ "confirmed_on": body.confirmed_on, "note": note }),
        )
        .await?;
    open_delivery_check(&mut tx, org_id, request_id, order_id).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
