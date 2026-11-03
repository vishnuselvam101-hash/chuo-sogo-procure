//! The order email (CEO specification 2026-11-03, step 4). After the factory approves and
//! records its own PO number, the system prepares the order to the chosen supplier. A person
//! on our side reads it and sends it; it goes through the same outbox as quote requests, so
//! it is only recorded (test mode) unless live supplier mail is configured. The factory still
//! pays the supplier directly; we never take the order or the money ourselves.

use axum::Json;
use axum::extract::State;
use chrono::{DateTime, NaiveDate, Utc};
use procure_domain::money::Money;
use procure_domain::rbac::Permission;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::api::common::{self, Audit, View, lock_request};
use crate::api::rfqs::{Recipient, queue_messages, require_operator};
use crate::db;
use crate::error::{ApiError, ApiJson, ApiPath, ApiResult, ErrorBody};
use crate::principal::Principal;

#[derive(sqlx::FromRow)]
struct OrderRow {
    request_id: Uuid,
    po_number: String,
    currency: String,
    shipping_minor: i64,
    total_minor: i64,
    promised_on: Option<NaiveDate>,
    delivery_confirmed_on: Option<NaiveDate>,
    factory_name: String,
    delivery_location: String,
    rfq_number: String,
    recipient_id: Uuid,
    supplier_name: String,
    supplier_reference: String,
}

#[derive(sqlx::FromRow)]
struct LineRow {
    line_no: i32,
    description: String,
    manufacturer: String,
    mpn: String,
    quantity: i32,
    unit: String,
    unit_price_minor: i64,
}

async fn load_order(tx: &mut db::Tx, order_id: Uuid) -> ApiResult<(OrderRow, Vec<LineRow>)> {
    let order = sqlx::query_as::<_, OrderRow>(
        "SELECT o.request_id, o.po_number, o.currency, o.shipping_minor, o.total_minor,
                o.promised_on, o.delivery_confirmed_on, g.name AS factory_name,
                r.delivery_location, f.number AS rfq_number, x.id AS recipient_id,
                s.name AS supplier_name, q.supplier_reference
         FROM purchase_orders o
         JOIN organisations g ON g.id = o.org_id
         JOIN procurement_requests r ON r.id = o.request_id
         JOIN recommendations rc ON rc.id = o.recommendation_id
         JOIN quotations q ON q.id = rc.quotation_id
         JOIN rfq_recipients x ON x.id = q.rfq_recipient_id
         JOIN rfqs f ON f.id = x.rfq_id
         JOIN suppliers s ON s.id = x.supplier_id
         WHERE o.id = $1",
    )
    .bind(order_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    let lines = sqlx::query_as::<_, LineRow>(
        "SELECT ri.line_no, ri.description,
                coalesce(ri.verified_manufacturer, ri.manufacturer) AS manufacturer,
                CASE WHEN btrim(oi.offered_mpn) <> '' THEN oi.offered_mpn
                     ELSE coalesce(ri.verified_mpn, ri.mpn) END AS mpn,
                oi.quantity, ri.unit, oi.unit_price_minor
         FROM order_items oi JOIN request_items ri ON ri.id = oi.request_item_id
         WHERE oi.order_id = $1 ORDER BY ri.line_no",
    )
    .bind(order_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok((order, lines))
}

/// Supplier-facing order text (Japanese first, English below each heading). It names the
/// factory, because the supplier delivers to it and invoices it directly, and the factory's
/// own PO number.
fn render_order(o: &OrderRow, lines: &[LineRow]) -> ApiResult<String> {
    let currency = common::currency(&o.currency)?;
    let money = |m: i64| crate::api::savings::amount_text(Money::from_minor(m, currency));
    let mut body = format!(
        "{supplier} 御中 / Dear {supplier},\n\n\
         お見積 {rfq}{reference} に基づき、下記のとおり注文いたします。\n\
         We order the items below, as quoted for {rfq}{reference}.\n\n\
         注文番号 / Purchase order number: {po}\n\
         発注者 / Ordered by: {factory}\n",
        supplier = o.supplier_name,
        rfq = o.rfq_number,
        reference = if o.supplier_reference.trim().is_empty() {
            String::new()
        } else {
            format!(" ({})", o.supplier_reference.trim())
        },
        po = o.po_number,
        factory = o.factory_name,
    );
    if !o.delivery_location.trim().is_empty() {
        body.push_str(&format!(
            "納品先 / Deliver to: {}\n",
            o.delivery_location.trim().replace('\n', " ")
        ));
    }
    if let Some(d) = o.delivery_confirmed_on.or(o.promised_on) {
        body.push_str(&format!("納期 / Delivery by: {d}\n"));
    }
    body.push('\n');
    for l in lines {
        body.push_str(&format!(
            "{}. {} | {} {} | {} {} × {}\n",
            l.line_no,
            l.description.replace('\n', " "),
            l.manufacturer,
            l.mpn,
            l.quantity,
            l.unit,
            money(l.unit_price_minor),
        ));
    }
    if o.shipping_minor > 0 {
        body.push_str(&format!("送料 / Shipping: {}\n", money(o.shipping_minor)));
    }
    body.push_str(&format!(
        "合計（税抜）/ Total before tax: {}\n\n\
         納期のご確認を、件名を変えずにご返信ください。\n\
         Please confirm the delivery date by replying without changing the subject.\n\
         請求書は {factory} 宛に注文番号 {po} を記載してお送りください（お支払いは {factory} から直接行われます）。\n\
         Please invoice {factory} directly, quoting PO {po}; {factory} pays you directly.\n",
        money(o.total_minor),
        factory = o.factory_name,
        po = o.po_number,
    ));
    Ok(body)
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct OrderEmailView {
    pub supplier_name: String,
    /// `draft` (prepared, not yet checked and sent), `recorded` (test mode), `queued`,
    /// `sent` or `failed`.
    pub status: String,
    /// `test`, `email` or `fax` once written to the outbox.
    pub channel: Option<String>,
    pub to_address: Option<String>,
    pub subject: Option<String>,
    pub body: String,
    pub checked_by_name: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub sent_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(sqlx::FromRow)]
struct Written {
    channel: String,
    to_address: Option<String>,
    subject: String,
    body: String,
    status: String,
    checked_by_name: Option<String>,
    created_at: DateTime<Utc>,
    sent_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/orders/{order_id}/order-email",
    tag = "orders",
    params(("org_id" = Uuid, Path), ("order_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "The order email: prepared text, or what was written to the outbox", body = OrderEmailView),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown order or not visible", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn get_order_email(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, order_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<OrderEmailView>> {
    let view = crate::api::orders::order_reader(&p, &state, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let visible: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM purchase_orders o JOIN procurement_requests r
                        ON r.id = o.request_id
                        WHERE o.id = $1 AND ($2::uuid IS NULL OR r.requested_by = $2))",
    )
    .bind(order_id)
    .bind(view.owner_filter())
    .fetch_one(&mut *tx)
    .await?;
    if !visible {
        return Err(ApiError::NotFound);
    }
    // The supplier of the factory's own order is visible to it (RFQ recipient).
    let (order, lines) = load_order(&mut tx, order_id).await?;
    let written = sqlx::query_as::<_, Written>(
        "SELECT o.channel, o.to_address, o.subject, o.body, o.status,
                u.display_name AS checked_by_name, o.created_at, o.sent_at, o.last_error
         FROM outbox o LEFT JOIN users u ON u.id = o.checked_by
         WHERE o.order_id = $1 AND o.kind = 'order'",
    )
    .bind(order_id)
    .fetch_optional(&mut *tx)
    .await?;
    let out = match written {
        Some(w) => OrderEmailView {
            supplier_name: order.supplier_name,
            status: w.status,
            channel: Some(w.channel),
            to_address: w.to_address,
            subject: Some(w.subject),
            body: w.body,
            checked_by_name: w.checked_by_name,
            created_at: Some(w.created_at),
            sent_at: w.sent_at,
            last_error: w.last_error,
        },
        None => OrderEmailView {
            body: render_order(&order, &lines)?,
            supplier_name: order.supplier_name,
            status: "draft".into(),
            channel: None,
            to_address: None,
            subject: None,
            checked_by_name: None,
            created_at: None,
            sent_at: None,
            last_error: None,
        },
    };
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct SendOrderEmailBody {
    /// The sender confirms having read the order against the approved quotation.
    pub checked: bool,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct SendOrderEmailResponse {
    /// `test` (recorded only) or `live` (queued for sending).
    pub channel: &'static str,
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/orders/{order_id}/order-email/send",
    tag = "orders",
    params(("org_id" = Uuid, Path), ("order_id" = Uuid, Path)),
    request_body = SendOrderEmailBody,
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Checked and written to the outbox (recorded in test mode)", body = SendOrderEmailResponse),
        (status = 409, description = "Already sent, not checked, request closed, or the supplier cannot be reached", body = ErrorBody),
        (status = 403, description = "Only our assigned staff send it", body = ErrorBody),
        (status = 404, description = "Unknown order", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn send_order_email(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, order_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<SendOrderEmailBody>,
) -> ApiResult<Json<SendOrderEmailResponse>> {
    require_operator(&p, &state, org_id, Permission::OperateRequests).await?;
    if !body.checked {
        return Err(ApiError::Conflict("order_email_not_checked"));
    }
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    db::enable_staff_scope(&mut tx).await?;
    let request_id: Uuid =
        sqlx::query_scalar("SELECT request_id FROM purchase_orders WHERE id = $1")
            .bind(order_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    let req = lock_request(&mut tx, request_id, View::All).await?;
    if matches!(req.status.as_str(), "closed" | "cancelled") {
        return Err(ApiError::Conflict("invalid_transition"));
    }
    let sent: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM outbox WHERE order_id = $1 AND kind = 'order')",
    )
    .bind(order_id)
    .fetch_one(&mut *tx)
    .await?;
    if sent {
        return Err(ApiError::Conflict("already_sent"));
    }
    let (order, lines) = load_order(&mut tx, order_id).await?;
    let text = render_order(&order, &lines)?;
    let recipients: Vec<Recipient> = sqlx::query_as(
        "SELECT x.id, s.name, s.email::text AS email, s.fax, s.rfq_channel
         FROM rfq_recipients x JOIN suppliers s ON s.id = x.supplier_id WHERE x.id = $1",
    )
    .bind(order.recipient_id)
    .fetch_all(&mut *tx)
    .await?;
    let number = format!("{} ({})", order.po_number, order.rfq_number);
    let channel = queue_messages(
        &state,
        &mut tx,
        org_id,
        &number,
        "order",
        &text,
        &recipients,
        Some((order_id, p.user_id)),
    )
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "order.email_sent",
            "purchase_order",
            order_id,
            json!({ "po_number": order.po_number, "channel": channel,
                    "supplier": order.supplier_name, "checked_by": p.user_id }),
        )
        .await?;
    crate::api::ops::close_auto_tasks(&mut tx, order.request_id, None, "send_order").await?;
    tx.commit().await?;
    Ok(Json(SendOrderEmailResponse { channel }))
}
