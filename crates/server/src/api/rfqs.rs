//! RFQs (test-mode dispatch only), quotation entry and deterministic comparison runs.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, NaiveDate, Utc};
use procure_domain::comparison::{self, Line, Quote, RequiredItem};
use procure_domain::decision;
use procure_domain::history;
use procure_domain::money::Money;
use procure_domain::rbac::{self, Access, Permission};
use procure_domain::workflow::{RequestStatus, Side};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::types::Json as DbJson;
use uuid::Uuid;

use crate::AppState;
use crate::api::common::{
    self, Audit, RequestRow, View, advance_if, lock_request, side, transition,
};
use crate::api::requests::{Created, MAX_ITEMS};
use crate::db;
use crate::error::{ApiError, ApiJson, ApiPath, ApiResult, ErrorBody};
use crate::principal::Principal;

pub const MAX_RECIPIENTS: usize = 20;

/// Staff assigned to the org with `perm`.
pub(crate) async fn require_operator(
    p: &Principal,
    state: &AppState,
    org_id: Uuid,
    perm: Permission,
) -> ApiResult<Access> {
    let access = p.require(state, org_id, perm).await?;
    if side(access)? == Side::Operator {
        Ok(access)
    } else {
        Err(ApiError::Forbidden)
    }
}

/// Reading RFQs, quotations and comparisons: assigned staff, or customer reviewers.
async fn require_quote_reader(p: &Principal, state: &AppState, org_id: Uuid) -> ApiResult<Access> {
    let access = p.access(state, org_id).await?;
    if access == Access::None {
        return Err(ApiError::NotFound);
    }
    let ok = [
        Permission::ManageRfqs,
        Permission::RecordQuotations,
        Permission::ReviewQuotations,
    ]
    .into_iter()
    .any(|perm| rbac::allows(access, perm));
    if ok {
        Ok(access)
    } else {
        Err(ApiError::Forbidden)
    }
}

/// Past prices are shown only to those who may read the purchase history (the same rule as
/// the price reference): managers of the records, or those who see all requests.
fn may_read_history(access: Access) -> bool {
    rbac::allows(access, Permission::ManagePurchaseHistory)
        || rbac::allows(access, Permission::ViewAllRequests)
}

/// Removes the past-price reference from a comparison for readers without history access.
/// The stored snapshot keeps it.
pub(crate) fn redact_price_checks(access: Access, results: &mut [ComparisonResult]) {
    if !may_read_history(access) {
        for r in results {
            r.price_checks.clear();
            r.price_alerts = 0;
            r.price_alert_bps = None;
            // Built from the history and the buying criteria.
            r.memo = None;
        }
    }
}

#[derive(sqlx::FromRow)]
struct RfqRow {
    id: Uuid,
    request_id: Uuid,
    number: String,
    status: String,
    checked: bool,
}

async fn lock_rfq(tx: &mut db::Tx, rfq_id: Uuid) -> ApiResult<RfqRow> {
    sqlx::query_as::<_, RfqRow>(
        "SELECT id, request_id, number, status, checked_at IS NOT NULL AS checked
         FROM rfqs WHERE id = $1 FOR UPDATE",
    )
    .bind(rfq_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreateRfqBody {
    pub supplier_ids: Vec<Uuid>,
    pub response_due: Option<NaiveDate>,
    /// Shown to suppliers. Must not contain the customer's identity.
    #[serde(default)]
    pub message: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/rfqs",
    tag = "rfqs",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    request_body = CreateRfqBody,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Draft RFQ created", body = Created),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown request or not assigned", body = ErrorBody),
        (status = 409, description = "Request is not in sourcing", body = ErrorBody),
        (status = 422, description = "Invalid recipients", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn create_rfq(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<CreateRfqBody>,
) -> ApiResult<(StatusCode, Json<Created>)> {
    require_operator(&p, &state, org_id, Permission::ManageRfqs).await?;
    let mut suppliers = body.supplier_ids.clone();
    suppliers.sort();
    suppliers.dedup();
    if suppliers.is_empty()
        || suppliers.len() > MAX_RECIPIENTS
        || suppliers.len() != body.supplier_ids.len()
    {
        return Err(ApiError::Validation("supplier_ids"));
    }
    let message = common::text(&body.message, 4000, true, "message")?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    db::enable_staff_scope(&mut tx).await?;
    let req = lock_request(&mut tx, request_id, View::All).await?;
    use RequestStatus::*;
    if !matches!(
        req.status()?,
        Sourcing | RfqSent | QuotesReceived | UnderComparison
    ) {
        return Err(ApiError::Conflict("invalid_transition"));
    }
    let verified: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM suppliers WHERE id = ANY($1) AND status = 'verified'",
    )
    .bind(&suppliers)
    .fetch_one(&mut *tx)
    .await?;
    if usize::try_from(verified).ok() != Some(suppliers.len()) {
        return Err(ApiError::Validation("supplier_not_verified"));
    }
    // The customer's items never go to a supplier the customer has rejected.
    let rejected: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM customer_supplier_approvals
                        WHERE org_id = $1 AND supplier_id = ANY($2) AND status = 'rejected')",
    )
    .bind(org_id)
    .bind(&suppliers)
    .fetch_one(&mut *tx)
    .await?;
    if rejected {
        return Err(ApiError::Validation("supplier_rejected_by_customer"));
    }
    let id = Uuid::now_v7();
    let number: String = sqlx::query_scalar("SELECT next_number($1, 'RFQ')")
        .bind(org_id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO rfqs (id, org_id, request_id, number, response_due, message, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(org_id)
    .bind(req.id)
    .bind(&number)
    .bind(body.response_due)
    .bind(&message)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    for s in &suppliers {
        sqlx::query(
            "INSERT INTO rfq_recipients (id, org_id, rfq_id, supplier_id) VALUES ($1, $2, $3, $4)",
        )
        .bind(Uuid::now_v7())
        .bind(org_id)
        .bind(id)
        .bind(s)
        .execute(&mut *tx)
        .await?;
    }
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "rfq.created",
            "rfq",
            id,
            json!({ "number": number, "request": req.id, "suppliers": suppliers }),
        )
        .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(Created { id, number })))
}

#[derive(sqlx::FromRow)]
struct RfqItem {
    line_no: i32,
    description: String,
    verified_manufacturer: Option<String>,
    verified_mpn: Option<String>,
    quantity: i32,
    unit: String,
}

/// Supplier-facing text. Refers to Chuo-Sogo's RFQ number only, never the customer.
fn render_rfq(number: &str, due: Option<NaiveDate>, message: &str, items: &[RfqItem]) -> String {
    let mut body = format!("Request for quotation {number}\n\n");
    if !message.is_empty() {
        body.push_str(message);
        body.push_str("\n\n");
    }
    for i in items {
        body.push_str(&format!(
            "{}. {} | {} {} | qty {} {}\n",
            i.line_no,
            i.description.replace('\n', " "),
            i.verified_manufacturer.as_deref().unwrap_or(""),
            i.verified_mpn.as_deref().unwrap_or(""),
            i.quantity,
            i.unit,
        ));
    }
    if let Some(d) = due {
        body.push_str(&format!("\nPlease respond by {d}.\n"));
    }
    body.push_str(&format!(
        "Please quote unit price, quantity, lead time and validity, quoting reference {number}.\n"
    ));
    body
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct DispatchResponse {
    /// `test`: messages are only recorded for manual export; `live`: queued and sent by email
    /// or FAX (server configured with `PROCURE_SUPPLIER_MAIL=live`).
    pub channel: &'static str,
    pub messages: usize,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct CheckBody {
    /// What was checked or corrected (optional).
    #[serde(default)]
    pub note: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/rfqs/{rfq_id}/check",
    tag = "rfqs",
    params(("org_id" = Uuid, Path), ("rfq_id" = Uuid, Path)),
    request_body = CheckBody,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Checked by our staff; the RFQ may now be sent"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown RFQ or not assigned", body = ErrorBody),
        (status = 409, description = "Already sent or already checked", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn check_rfq(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, rfq_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<CheckBody>,
) -> ApiResult<StatusCode> {
    require_operator(&p, &state, org_id, Permission::ManageRfqs).await?;
    let note = common::text(&body.note, 2000, true, "note")?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    db::enable_staff_scope(&mut tx).await?;
    let rfq = lock_rfq(&mut tx, rfq_id).await?;
    if rfq.status != "draft" {
        return Err(ApiError::Conflict("already_sent"));
    }
    if rfq.checked {
        return Err(ApiError::Conflict("already_checked"));
    }
    sqlx::query(
        "UPDATE rfqs SET checked_by = $2, checked_at = now(), check_note = $3 WHERE id = $1",
    )
    .bind(rfq.id)
    .bind(p.user_id)
    .bind(&note)
    .execute(&mut *tx)
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "rfq.checked",
            "rfq",
            rfq.id,
            json!({ "number": rfq.number, "note": note }),
        )
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/rfqs/{rfq_id}/dispatch",
    tag = "rfqs",
    params(("org_id" = Uuid, Path), ("rfq_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "RFQ marked sent; supplier messages recorded in the test outbox", body = DispatchResponse),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown RFQ or not assigned", body = ErrorBody),
        (status = 409, description = "Already sent", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn dispatch_rfq(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, rfq_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<DispatchResponse>> {
    require_operator(&p, &state, org_id, Permission::ManageRfqs).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    db::enable_staff_scope(&mut tx).await?;
    let rfq = lock_rfq(&mut tx, rfq_id).await?;
    let req = lock_request(&mut tx, rfq.request_id, View::All).await?;
    if rfq.status != "draft" {
        return Err(ApiError::Conflict("already_sent"));
    }
    if !rfq.checked {
        return Err(ApiError::Conflict("rfq_not_checked"));
    }
    let (due, message): (Option<NaiveDate>, String) =
        sqlx::query_as("SELECT response_due, message FROM rfqs WHERE id = $1")
            .bind(rfq.id)
            .fetch_one(&mut *tx)
            .await?;
    let items = sqlx::query_as::<_, RfqItem>(
        "SELECT line_no, description, verified_manufacturer, verified_mpn, quantity, unit
         FROM request_items WHERE request_id = $1 ORDER BY line_no",
    )
    .bind(req.id)
    .fetch_all(&mut *tx)
    .await?;
    let body = render_rfq(&rfq.number, due, &message, &items);
    let recipients: Vec<Recipient> = sqlx::query_as(
        "SELECT x.id, s.name, s.email::text AS email, s.fax, s.rfq_channel
         FROM rfq_recipients x JOIN suppliers s ON s.id = x.supplier_id
         WHERE x.rfq_id = $1 ORDER BY x.id",
    )
    .bind(rfq.id)
    .fetch_all(&mut *tx)
    .await?;
    let channel = queue_messages(
        &state,
        &mut tx,
        org_id,
        &rfq.number,
        "rfq",
        &body,
        &recipients,
        None,
    )
    .await?;
    sqlx::query("UPDATE rfq_recipients SET status = 'exported' WHERE rfq_id = $1")
        .bind(rfq.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE rfqs SET status = 'sent', sent_at = now() WHERE id = $1")
        .bind(rfq.id)
        .execute(&mut *tx)
        .await?;
    let audit = Audit { org_id, p: &p };
    audit
        .record(
            &mut tx,
            "rfq.dispatched",
            "rfq",
            rfq.id,
            json!({ "number": rfq.number, "channel": channel, "messages": recipients.len() }),
        )
        .await?;
    advance_if(
        &mut tx,
        &audit,
        &req,
        RequestStatus::Sourcing,
        RequestStatus::RfqSent,
        "request.rfq_sent",
    )
    .await?;
    // Chase the suppliers by the response date (end of that day in the customer's
    // timezone), or in 72 hours.
    let chase_due = match due {
        Some(d) => crate::api::ops::end_of_local_day(&mut tx, org_id, d).await?,
        None => {
            chrono::Utc::now()
                + chrono::Duration::hours(i64::from(
                    crate::api::admin::load_settings(&mut tx, org_id)
                        .await?
                        .chase_default_hours,
                ))
        }
    };
    crate::api::ops::open_auto_task(
        &mut tx,
        org_id,
        req.id,
        Some(rfq.id),
        "chase_quotes",
        &format!("Collect quotations for {}", rfq.number),
        chase_due,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(DispatchResponse {
        channel,
        messages: recipients.len(),
    }))
}

#[derive(sqlx::FromRow)]
pub(crate) struct Recipient {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) email: Option<String>,
    pub(crate) fax: Option<String>,
    pub(crate) rfq_channel: String,
}

/// Records (test mode) or queues (live) one message per recipient, each with its reply
/// token. Live refuses the whole batch if any recipient cannot be reached, so nobody is
/// left out silently. Returns "test" or "live". `order` is (order id, the person who checked
/// it) for an order email.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn queue_messages(
    state: &AppState,
    tx: &mut db::Tx,
    org_id: Uuid,
    number: &str,
    kind: &str,
    body: &str,
    recipients: &[Recipient],
    order: Option<(Uuid, Uuid)>,
) -> ApiResult<&'static str> {
    let mut rows = Vec::with_capacity(recipients.len());
    for r in recipients {
        let (channel, to) = match &state.supplier_mail {
            None => ("test", r.email.clone()),
            Some(m) => {
                let (c, to) = m
                    .address(&r.rfq_channel, r.email.as_deref(), r.fax.as_deref())
                    .map_err(|code| {
                        tracing::info!(supplier = %r.name, code, "recipient cannot be reached");
                        ApiError::Conflict(code)
                    })?;
                (c, Some(to))
            }
        };
        rows.push((r.id, channel, to));
    }
    for (recipient, channel, to) in rows {
        let token = crate::supplier_mail::reply_token();
        sqlx::query(
            "INSERT INTO outbox (id, org_id, rfq_recipient_id, channel, to_address, subject, body,
                                 kind, reply_token, status, next_attempt_at, order_id, checked_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9,
                     CASE WHEN $4 = 'test' THEN 'recorded' ELSE 'queued' END,
                     CASE WHEN $4 = 'test' THEN NULL ELSE now() END, $10, $11)",
        )
        .bind(Uuid::now_v7())
        .bind(org_id)
        .bind(recipient)
        .bind(channel)
        .bind(to)
        .bind(crate::supplier_mail::subject(kind, number, &token))
        .bind(body)
        .bind(kind)
        .bind(&token)
        .bind(order.map(|o| o.0))
        .bind(order.map(|o| o.1))
        .execute(&mut **tx)
        .await?;
    }
    Ok(if state.supplier_mail.is_some() {
        "live"
    } else {
        "test"
    })
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct RemindResponse {
    pub channel: &'static str,
    /// Suppliers reminded (those that have not replied or declined).
    pub messages: usize,
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/rfqs/{rfq_id}/remind",
    tag = "rfqs",
    params(("org_id" = Uuid, Path), ("rfq_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Reminders recorded (test mode) or queued", body = RemindResponse),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown RFQ or not assigned", body = ErrorBody),
        (status = 409, description = "Not sent, nobody left to remind, or reminded in the last 20 hours", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn remind_rfq(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, rfq_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<RemindResponse>> {
    require_operator(&p, &state, org_id, Permission::ManageRfqs).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    db::enable_staff_scope(&mut tx).await?;
    let rfq = lock_rfq(&mut tx, rfq_id).await?;
    if rfq.status != "sent" {
        return Err(ApiError::Conflict("rfq_not_sent"));
    }
    let recent: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM outbox o JOIN rfq_recipients r ON r.id = o.rfq_recipient_id
                        WHERE r.rfq_id = $1 AND o.kind = 'reminder'
                          AND o.created_at > now() - interval '20 hours')",
    )
    .bind(rfq.id)
    .fetch_one(&mut *tx)
    .await?;
    if recent {
        return Err(ApiError::Conflict("reminded_recently"));
    }
    let recipients: Vec<Recipient> = sqlx::query_as(
        "SELECT x.id, s.name, s.email::text AS email, s.fax, s.rfq_channel
         FROM rfq_recipients x JOIN suppliers s ON s.id = x.supplier_id
         WHERE x.rfq_id = $1 AND x.status IN ('pending', 'exported') ORDER BY x.id",
    )
    .bind(rfq.id)
    .fetch_all(&mut *tx)
    .await?;
    if recipients.is_empty() {
        return Err(ApiError::Conflict("nobody_to_remind"));
    }
    let (due, message): (Option<NaiveDate>, String) =
        sqlx::query_as("SELECT response_due, message FROM rfqs WHERE id = $1")
            .bind(rfq.id)
            .fetch_one(&mut *tx)
            .await?;
    let items = sqlx::query_as::<_, RfqItem>(
        "SELECT line_no, description, verified_manufacturer, verified_mpn, quantity, unit
         FROM request_items WHERE request_id = $1 ORDER BY line_no",
    )
    .bind(rfq.request_id)
    .fetch_all(&mut *tx)
    .await?;
    let body = format!(
        "先日お送りした見積依頼へのご回答をお待ちしております。件名を変えずにご返信ください。\n\
         We are still waiting for your quotation for the request below. Please reply without changing the subject.\n\n{}",
        render_rfq(&rfq.number, due, &message, &items)
    );
    let channel = queue_messages(
        &state,
        &mut tx,
        org_id,
        &rfq.number,
        "reminder",
        &body,
        &recipients,
        None,
    )
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "rfq.reminded",
            "rfq",
            rfq.id,
            json!({ "number": rfq.number, "channel": channel, "messages": recipients.len() }),
        )
        .await?;
    tx.commit().await?;
    Ok(Json(RemindResponse {
        channel,
        messages: recipients.len(),
    }))
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct OutboxMessage {
    pub id: Uuid,
    pub supplier_id: Uuid,
    pub supplier_name: String,
    pub channel: String,
    pub to_address: Option<String>,
    pub subject: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
    /// rfq or reminder.
    pub kind: String,
    /// recorded (test mode), queued, sent or failed.
    pub status: String,
    pub sent_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/rfqs/{rfq_id}/outbox",
    tag = "rfqs",
    params(("org_id" = Uuid, Path), ("rfq_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Recorded supplier messages for manual export", body = [OutboxMessage]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown RFQ or not assigned", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn rfq_outbox(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, rfq_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<Vec<OutboxMessage>>> {
    require_operator(&p, &state, org_id, Permission::ManageRfqs).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    db::enable_staff_scope(&mut tx).await?;
    let rows = sqlx::query_as::<_, OutboxMessage>(
        "SELECT o.id, x.supplier_id, s.name AS supplier_name, o.channel, o.to_address, o.subject,
                o.body, o.created_at, o.kind, o.status, o.sent_at, o.last_error
         FROM outbox o JOIN rfq_recipients x ON x.id = o.rfq_recipient_id
         JOIN suppliers s ON s.id = x.supplier_id
         WHERE x.rfq_id = $1 ORDER BY s.name, o.created_at, o.id",
    )
    .bind(rfq_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct QuotationLineInput {
    pub request_item_id: Uuid,
    pub unit_price_minor: i64,
    pub quantity: u32,
    pub lead_time_days: Option<u32>,
    #[serde(default)]
    pub offered_manufacturer: String,
    #[serde(default)]
    pub offered_mpn: String,
    /// The supplier offers a different product than the verified one.
    #[serde(default)]
    pub alternative: bool,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct QuotationInput {
    pub supplier_id: Uuid,
    #[serde(default)]
    pub supplier_reference: String,
    pub currency: String,
    #[serde(default)]
    pub shipping_minor: i64,
    pub valid_until: Option<NaiveDate>,
    /// Defaults to now.
    pub received_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub notes: String,
    pub lines: Vec<QuotationLineInput>,
    /// The AI suggestion the operator started from (marked accepted; values are the operator's).
    pub ai_suggestion_id: Option<Uuid>,
    /// The supplier's file this quotation was read from (`reply-text`), kept as received.
    pub source_document_id: Option<Uuid>,
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/rfqs/{rfq_id}/quotations",
    tag = "rfqs",
    params(("org_id" = Uuid, Path), ("rfq_id" = Uuid, Path)),
    request_body = QuotationInput,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Quotation recorded (replaces the supplier's previous one)", body = crate::api::suppliers::IdResponse),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown RFQ or not assigned", body = ErrorBody),
        (status = 409, description = "RFQ not sent", body = ErrorBody),
        (status = 422, description = "Invalid quotation", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn record_quotation(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, rfq_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<QuotationInput>,
) -> ApiResult<(StatusCode, Json<crate::api::suppliers::IdResponse>)> {
    require_operator(&p, &state, org_id, Permission::RecordQuotations).await?;
    let currency = common::currency(&body.currency)?;
    if body.shipping_minor < 0 {
        return Err(ApiError::Validation("shipping_minor"));
    }
    if body.lines.is_empty() || body.lines.len() > MAX_ITEMS {
        return Err(ApiError::Validation("lines"));
    }
    for l in &body.lines {
        if l.unit_price_minor < 0 {
            return Err(ApiError::Validation("unit_price_minor"));
        }
        if l.quantity == 0 || l.quantity > 1_000_000_000 {
            return Err(ApiError::Validation("quantity"));
        }
        if l.lead_time_days.is_some_and(|d| d > 3650) {
            return Err(ApiError::Validation("lead_time_days"));
        }
    }
    let reference = common::text(&body.supplier_reference, 100, false, "supplier_reference")?;
    let notes = common::text(&body.notes, 2000, true, "notes")?;

    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let rfq = lock_rfq(&mut tx, rfq_id).await?;
    let req = lock_request(&mut tx, rfq.request_id, View::All).await?;
    if rfq.status != "sent" {
        return Err(ApiError::Conflict("rfq_not_sent"));
    }
    // Once a recommendation exists, quotations are frozen: the customer decides on exactly
    // what was compared (withdraw the recommendation first to take a revision into account).
    use RequestStatus::*;
    if !matches!(req.status()?, RfqSent | QuotesReceived | UnderComparison) {
        return Err(ApiError::Conflict("quotations_frozen"));
    }
    let recipient: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM rfq_recipients WHERE rfq_id = $1 AND supplier_id = $2")
            .bind(rfq.id)
            .bind(body.supplier_id)
            .fetch_optional(&mut *tx)
            .await?;
    let recipient = recipient.ok_or(ApiError::Validation("supplier_not_invited"))?;
    let item_ids: Vec<Uuid> = body.lines.iter().map(|l| l.request_item_id).collect();
    let known: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT id) FROM request_items WHERE request_id = $1 AND id = ANY($2)",
    )
    .bind(req.id)
    .bind(&item_ids)
    .fetch_one(&mut *tx)
    .await?;
    let mut distinct = item_ids.clone();
    distinct.sort();
    distinct.dedup();
    if usize::try_from(known).ok() != Some(distinct.len()) {
        return Err(ApiError::Validation("unknown_item"));
    }
    // The source file must belong to this RFQ and, when it names one, to this supplier.
    if let Some(d) = body.source_document_id {
        let ok: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM supplier_documents
                            WHERE id = $1 AND rfq_id = $2
                              AND (supplier_id IS NULL OR supplier_id = $3))",
        )
        .bind(d)
        .bind(rfq.id)
        .bind(body.supplier_id)
        .fetch_one(&mut *tx)
        .await?;
        if !ok {
            return Err(ApiError::Validation("source_document_id"));
        }
    }
    let superseded: Option<Uuid> = sqlx::query_scalar(
        "UPDATE quotations SET superseded_at = now()
         WHERE rfq_recipient_id = $1 AND superseded_at IS NULL RETURNING id",
    )
    .bind(recipient)
    .fetch_optional(&mut *tx)
    .await?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO quotations (id, org_id, rfq_id, rfq_recipient_id, supplier_reference, currency,
                                 shipping_minor, valid_until, received_at, notes, recorded_by,
                                 ai_suggestion_id, source_document_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, coalesce($9, now()), $10, $11, $12, $13)",
    )
    .bind(id)
    .bind(org_id)
    .bind(rfq.id)
    .bind(recipient)
    .bind(&reference)
    .bind(currency.code())
    .bind(body.shipping_minor)
    .bind(body.valid_until)
    .bind(body.received_at)
    .bind(&notes)
    .bind(p.user_id)
    .bind(body.ai_suggestion_id)
    .bind(body.source_document_id)
    .execute(&mut *tx)
    .await?;
    for l in &body.lines {
        sqlx::query(
            "INSERT INTO quotation_lines (id, org_id, quotation_id, request_item_id, unit_price_minor,
                                          quantity, lead_time_days, offered_manufacturer,
                                          offered_mpn, alternative)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(Uuid::now_v7())
        .bind(org_id)
        .bind(id)
        .bind(l.request_item_id)
        .bind(l.unit_price_minor)
        .bind(i32::try_from(l.quantity).map_err(|_| ApiError::Validation("quantity"))?)
        .bind(l.lead_time_days.map(i32::try_from).transpose().map_err(|_| ApiError::Validation("lead_time_days"))?)
        .bind(common::text(&l.offered_manufacturer, 200, false, "offered_manufacturer")?)
        .bind(common::text(&l.offered_mpn, 100, false, "offered_mpn")?)
        .bind(l.alternative)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("UPDATE rfq_recipients SET status = 'responded' WHERE id = $1")
        .bind(recipient)
        .execute(&mut *tx)
        .await?;
    let audit = Audit { org_id, p: &p };
    if let Some(suggestion) = body.ai_suggestion_id {
        crate::api::ai::accept_quotation_suggestion(
            &mut tx,
            &audit,
            suggestion,
            rfq.id,
            body.supplier_id,
            id,
        )
        .await?;
    }
    audit
        .record(
            &mut tx,
            "quotation.recorded",
            "quotation",
            id,
            json!({ "rfq": rfq.number, "supplier": body.supplier_id, "currency": currency.code(),
                    "lines": body.lines.len(), "supersedes": superseded }),
        )
        .await?;
    advance_if(
        &mut tx,
        &audit,
        &req,
        RequestStatus::RfqSent,
        RequestStatus::QuotesReceived,
        "request.quotes_received",
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(crate::api::suppliers::IdResponse { id }),
    ))
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct QuotationLineView {
    pub request_item_id: Uuid,
    pub unit_price_minor: i64,
    pub quantity: i32,
    pub lead_time_days: Option<i32>,
    pub offered_manufacturer: String,
    pub offered_mpn: String,
    pub alternative: bool,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct QuotationView {
    pub id: Uuid,
    pub supplier_id: Uuid,
    pub supplier_name: Option<String>,
    pub supplier_reference: String,
    pub currency: String,
    pub shipping_minor: i64,
    pub valid_until: Option<NaiveDate>,
    pub received_at: DateTime<Utc>,
    #[sqlx(skip)]
    pub lines: Vec<QuotationLineView>,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct RecipientView {
    pub supplier_id: Uuid,
    pub supplier_name: Option<String>,
    pub status: String,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct RfqHeader {
    pub id: Uuid,
    pub request_id: Uuid,
    pub number: String,
    pub status: String,
    pub response_due: Option<NaiveDate>,
    pub message: String,
    pub created_at: DateTime<Utc>,
    pub sent_at: Option<DateTime<Utc>>,
    /// Our staff check before the RFQ goes out (required for dispatch).
    pub checked_at: Option<DateTime<Utc>>,
    pub checked_by_name: Option<String>,
    pub check_note: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct RfqDetail {
    pub rfq: RfqHeader,
    pub recipients: Vec<RecipientView>,
    /// Current (not superseded) quotations.
    pub quotations: Vec<QuotationView>,
    /// Most recent comparison run of this RFQ, if any.
    pub latest_comparison_id: Option<Uuid>,
    /// Supplier files received for this RFQ (uploaded, or read from the mailbox), kept as
    /// received. Shown to our staff only.
    pub documents: Vec<DocumentView>,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct DocumentView {
    pub id: Uuid,
    pub supplier_id: Option<Uuid>,
    pub supplier_name: Option<String>,
    /// upload, email or fax.
    pub channel: String,
    pub file_name: String,
    pub received_at: DateTime<Utc>,
    /// A quotation was recorded from it.
    pub used: bool,
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/rfqs/{rfq_id}",
    tag = "rfqs",
    params(("org_id" = Uuid, Path), ("rfq_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "RFQ with recipients and current quotations", body = RfqDetail),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown RFQ", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn get_rfq(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, rfq_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<RfqDetail>> {
    let access = require_quote_reader(&p, &state, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let rfq = sqlx::query_as::<_, RfqHeader>(
        "SELECT f.id, f.request_id, f.number, f.status, f.response_due, f.message, f.created_at,
                f.sent_at, f.checked_at, u.display_name AS checked_by_name, f.check_note
         FROM rfqs f LEFT JOIN users u ON u.id = f.checked_by WHERE f.id = $1",
    )
    .bind(rfq_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    let recipients = sqlx::query_as::<_, RecipientView>(
        "SELECT x.supplier_id, s.name AS supplier_name, x.status
         FROM rfq_recipients x LEFT JOIN suppliers s ON s.id = x.supplier_id
         WHERE x.rfq_id = $1 ORDER BY s.name, x.id",
    )
    .bind(rfq_id)
    .fetch_all(&mut *tx)
    .await?;
    let mut quotations = sqlx::query_as::<_, QuotationView>(
        "SELECT q.id, x.supplier_id, s.name AS supplier_name, q.supplier_reference, q.currency,
                q.shipping_minor, q.valid_until, q.received_at
         FROM quotations q JOIN rfq_recipients x ON x.id = q.rfq_recipient_id
         LEFT JOIN suppliers s ON s.id = x.supplier_id
         WHERE q.rfq_id = $1 AND q.superseded_at IS NULL ORDER BY s.name, q.id",
    )
    .bind(rfq_id)
    .fetch_all(&mut *tx)
    .await?;
    for q in &mut quotations {
        q.lines = sqlx::query_as::<_, QuotationLineView>(
            "SELECT l.request_item_id, l.unit_price_minor, l.quantity, l.lead_time_days,
                    l.offered_manufacturer, l.offered_mpn, l.alternative
             FROM quotation_lines l JOIN request_items i ON i.id = l.request_item_id
             WHERE l.quotation_id = $1 ORDER BY i.line_no",
        )
        .bind(q.id)
        .fetch_all(&mut *tx)
        .await?;
    }
    let latest_comparison_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM comparison_runs WHERE rfq_id = $1 ORDER BY ran_at DESC, id DESC LIMIT 1",
    )
    .bind(rfq_id)
    .fetch_optional(&mut *tx)
    .await?;
    let documents = if side(access)? == Side::Operator {
        sqlx::query_as::<_, DocumentView>(
            "SELECT d.id, d.supplier_id, s.name AS supplier_name, d.channel, d.file_name,
                    d.received_at,
                    EXISTS (SELECT 1 FROM quotations q WHERE q.source_document_id = d.id) AS used
             FROM supplier_documents d LEFT JOIN suppliers s ON s.id = d.supplier_id
             WHERE d.rfq_id = $1 ORDER BY d.received_at DESC, d.id",
        )
        .bind(rfq_id)
        .fetch_all(&mut *tx)
        .await?
    } else {
        Vec::new()
    };
    tx.commit().await?;
    Ok(Json(RfqDetail {
        rfq,
        recipients,
        quotations,
        latest_comparison_id,
        documents,
    }))
}

/// One quotation's evaluation inside a stored comparison run.
#[derive(Serialize, Deserialize, Clone, utoipa::ToSchema)]
pub struct ComparisonResult {
    pub quotation_id: Uuid,
    pub supplier_id: Uuid,
    pub supplier_name: String,
    pub currency: String,
    /// `compliant`, `needs_review`, `incomplete` or `non_compliant`.
    pub compliance: String,
    pub reasons: Vec<String>,
    /// In the request currency; absent when it cannot be computed without FX.
    pub total_minor: Option<i64>,
    pub max_lead_time_days: Option<u32>,
    /// 1-based among compliant quotations.
    pub rank: Option<u32>,
    /// Reference only: each quoted line against the customer's last purchase of the item.
    /// Never affects compliance or rank. Empty for readers without purchase-history access.
    #[serde(default)]
    pub price_checks: Vec<PriceCheck>,
    /// Lines whose change is above the customer's alert line.
    #[serde(default)]
    pub price_alerts: u32,
    /// The customer's alert line when the comparison ran (`buying_criteria.price_alert_bps`).
    #[serde(default)]
    pub price_alert_bps: Option<i64>,
    /// The decision memo (customer's priorities, risks, evidence). A recommendation for a
    /// person, never a decision; absent in runs stored before it existed.
    #[serde(default)]
    pub memo: Option<DecisionMemo>,
}

/// One factor of the memo's score.
#[derive(Serialize, Deserialize, Clone, utoipa::ToSchema)]
pub struct MemoFactor {
    /// `price`, `lead_time`, `existing_supplier` or `domestic`.
    pub factor: String,
    pub weight: u32,
    /// 0–100.
    pub points: u32,
}

/// How one quotation fares against the customer's own criteria (the LAYA method: evidence,
/// scoring, risk penalties, reasons). Deterministic; uses only this customer's data.
#[derive(Serialize, Deserialize, Clone, utoipa::ToSchema)]
pub struct DecisionMemo {
    /// 0–1000; null when it cannot be recommended (excluded, not compliant, no comparable total).
    pub score: Option<u32>,
    /// 1 = recommended.
    pub rank: Option<u32>,
    pub factors: Vec<MemoFactor>,
    /// `excluded_supplier`, `not_compliant`, `total_not_comparable`, `lead_time_unknown`,
    /// `origin_unknown`, `price_above_alert`, `over_max_lead_time`, `quality_not_assessed`,
    /// `close_call`.
    pub cautions: Vec<String>,
    pub preferred_supplier: bool,
    /// Purchases from this supplier in the customer's history.
    pub past_purchases: u32,
}

/// A quoted line next to the last price the customer paid for the same item.
#[derive(Serialize, Deserialize, Clone, utoipa::ToSchema)]
pub struct PriceCheck {
    pub request_item_id: Uuid,
    /// Quoted unit price, in the quotation currency.
    pub quoted_unit_price_minor: i64,
    /// Thousandths of a minor unit of `last_currency`.
    pub last_unit_price_milli: i64,
    pub last_currency: String,
    pub last_purchased_on: NaiveDate,
    pub last_supplier_name: String,
    /// Change against the last price in basis points (+1250 = 12.5 % dearer); null when the
    /// currencies differ (no FX).
    pub change_bps: Option<i64>,
    pub above_alert: bool,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct ComparisonView {
    pub id: Uuid,
    pub rfq_id: Uuid,
    pub currency: String,
    pub as_of: NaiveDate,
    #[schema(value_type = Vec<ComparisonResult>)]
    pub results: DbJson<Vec<ComparisonResult>>,
    pub ran_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct QuoteRow {
    id: Uuid,
    supplier_id: Uuid,
    supplier_name: String,
    currency: String,
    shipping_minor: i64,
    expired: bool,
    country: String,
}

#[derive(sqlx::FromRow)]
struct CompareItemRow {
    id: Uuid,
    quantity: i32,
    line_no: i32,
    description: String,
    mpn: String,
    verified_mpn: Option<String>,
}

#[derive(sqlx::FromRow)]
struct LastPurchase {
    item_key: String,
    purchased_on: NaiveDate,
    supplier_name: String,
    unit_price_milli: i64,
    currency: String,
}

#[derive(sqlx::FromRow)]
struct LineRow {
    quotation_id: Uuid,
    request_item_id: Uuid,
    unit_price_minor: i64,
    quantity: i32,
    lead_time_days: Option<i32>,
    alternative: bool,
}

fn to_u32(v: i32) -> ApiResult<u32> {
    u32::try_from(v).map_err(|_| ApiError::internal(std::io::Error::other("negative quantity")))
}

/// Runs the deterministic comparison over the RFQ's current quotations and stores it.
async fn compare_rfq(
    tx: &mut db::Tx,
    org_id: Uuid,
    rfq_id: Uuid,
    req: &RequestRow,
    user: Uuid,
) -> ApiResult<ComparisonView> {
    let currency = req.currency()?;
    let as_of: NaiveDate = sqlx::query_scalar(
        "SELECT (now() AT TIME ZONE timezone)::date FROM organisations WHERE id = $1",
    )
    .bind(org_id)
    .fetch_one(&mut **tx)
    .await?;
    let items = sqlx::query_as::<_, CompareItemRow>(
        "SELECT id, quantity, line_no, description, mpn, verified_mpn FROM request_items
         WHERE request_id = $1 ORDER BY line_no",
    )
    .bind(req.id)
    .fetch_all(&mut **tx)
    .await?;
    let required = items
        .iter()
        .map(|i| {
            Ok(RequiredItem {
                id: i.id,
                quantity: to_u32(i.quantity)?,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let quotes = sqlx::query_as::<_, QuoteRow>(
        "SELECT q.id, x.supplier_id, s.name AS supplier_name, q.currency, q.shipping_minor,
                coalesce(q.valid_until < $2, false) AS expired, s.country
         FROM quotations q JOIN rfq_recipients x ON x.id = q.rfq_recipient_id
         JOIN suppliers s ON s.id = x.supplier_id
         WHERE q.rfq_id = $1 AND q.superseded_at IS NULL ORDER BY q.id",
    )
    .bind(rfq_id)
    .bind(as_of)
    .fetch_all(&mut **tx)
    .await?;
    if quotes.is_empty() {
        return Err(ApiError::Conflict("no_quotations"));
    }
    let lines = sqlx::query_as::<_, LineRow>(
        "SELECT l.quotation_id, l.request_item_id, l.unit_price_minor, l.quantity, l.lead_time_days,
                l.alternative
         FROM quotation_lines l JOIN quotations q ON q.id = l.quotation_id
         WHERE q.rfq_id = $1 AND q.superseded_at IS NULL",
    )
    .bind(rfq_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut domain_quotes = Vec::with_capacity(quotes.len());
    for q in &quotes {
        let qc = common::currency(&q.currency)?;
        let mut ls = Vec::new();
        for l in lines.iter().filter(|l| l.quotation_id == q.id) {
            ls.push(Line {
                item: l.request_item_id,
                unit_price: Money::from_minor(l.unit_price_minor, qc),
                quantity: to_u32(l.quantity)?,
                lead_time_days: l.lead_time_days.map(to_u32).transpose()?,
                alternative: l.alternative,
            });
        }
        domain_quotes.push(Quote {
            id: q.id,
            shipping: Money::from_minor(q.shipping_minor, qc),
            expired: q.expired,
            lines: ls,
        });
    }
    let evaluations = comparison::compare(currency, &required, &domain_quotes);

    // Reference only, computed after the evaluation: the last price paid per item (same key
    // as the price reference: a verified part number wins over the typed one).
    let alert_bps: Option<i32> =
        sqlx::query_scalar("SELECT price_alert_bps FROM buying_criteria WHERE org_id = $1")
            .bind(org_id)
            .fetch_optional(&mut **tx)
            .await?
            .flatten();
    let alert_bps = alert_bps.map(i64::from);
    let keys: std::collections::HashMap<Uuid, String> = items
        .iter()
        .filter_map(|i| {
            history::item_key(i.verified_mpn.as_deref().unwrap_or(&i.mpn), &i.description)
                .map(|k| (i.id, k))
        })
        .collect();
    let own: Vec<String> = keys.values().cloned().collect();
    // Items confirmed as the same (aliases) count as one.
    let groups = crate::api::history::key_groups(tx, &own).await?;
    let wanted: Vec<String> = groups.values().flatten().cloned().collect();
    let by_key: std::collections::HashMap<String, LastPurchase> =
        sqlx::query_as::<_, LastPurchase>(
            "SELECT DISTINCT ON (item_key) item_key, purchased_on, supplier_name,
                    unit_price_milli, currency
             FROM purchase_history WHERE item_key = ANY($1)
             ORDER BY item_key, purchased_on DESC, id DESC",
        )
        .bind(&wanted)
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .map(|l| (l.item_key.clone(), l))
        .collect();
    // The most recent purchase across each item's group (ties: the smaller key, so the
    // choice is stable).
    let last: std::collections::HashMap<&String, &LastPurchase> = own
        .iter()
        .filter_map(|k| {
            groups
                .get(k)?
                .iter()
                .filter_map(|g| by_key.get(g))
                .max_by(|a, b| {
                    a.purchased_on
                        .cmp(&b.purchased_on)
                        .then_with(|| b.item_key.cmp(&a.item_key))
                })
                .map(|l| (k, l))
        })
        .collect();
    let line_no: std::collections::HashMap<Uuid, i32> =
        items.iter().map(|i| (i.id, i.line_no)).collect();
    let price_checks_for = |q: &QuoteRow| -> ApiResult<Vec<PriceCheck>> {
        let qc = common::currency(&q.currency)?;
        let mut ql: Vec<&LineRow> = lines.iter().filter(|l| l.quotation_id == q.id).collect();
        ql.sort_by_key(|l| line_no.get(&l.request_item_id).copied().unwrap_or(i32::MAX));
        let mut out = Vec::new();
        for l in ql {
            let Some(p) = keys.get(&l.request_item_id).and_then(|k| last.get(k)) else {
                continue;
            };
            let lc = common::currency(&p.currency)?;
            let change = history::change_bps(p.unit_price_milli, lc, l.unit_price_minor, qc);
            out.push(PriceCheck {
                request_item_id: l.request_item_id,
                quoted_unit_price_minor: l.unit_price_minor,
                last_unit_price_milli: p.unit_price_milli,
                last_currency: p.currency.clone(),
                last_purchased_on: p.purchased_on,
                last_supplier_name: p.supplier_name.clone(),
                change_bps: change,
                above_alert: history::above_alert(change, alert_bps),
            });
        }
        Ok(out)
    };

    let mut results = Vec::with_capacity(evaluations.len());
    for e in &evaluations {
        let Some(q) = quotes.iter().find(|q| q.id == e.quote) else {
            continue;
        };
        let price_checks = price_checks_for(q)?;
        let price_alerts = u32::try_from(price_checks.iter().filter(|c| c.above_alert).count())
            .unwrap_or(u32::MAX);
        results.push(ComparisonResult {
            quotation_id: q.id,
            supplier_id: q.supplier_id,
            supplier_name: q.supplier_name.clone(),
            currency: q.currency.clone(),
            compliance: e.compliance.code().to_string(),
            reasons: e.reasons.iter().map(|r| r.code().to_string()).collect(),
            total_minor: e.total.map(Money::minor),
            max_lead_time_days: e.max_lead_time_days,
            rank: e.rank,
            price_checks,
            price_alerts,
            price_alert_bps: alert_bps,
            memo: None,
        });
    }
    attach_memos(tx, &quotes, &mut results).await?;
    let id = Uuid::now_v7();
    let quotation_ids: Vec<Uuid> = quotes.iter().map(|q| q.id).collect();
    let view = sqlx::query_as::<_, ComparisonView>(
        "INSERT INTO comparison_runs (id, org_id, rfq_id, currency, as_of, quotation_ids, results,
                                      ran_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING id, rfq_id, currency, as_of, results, ran_at",
    )
    .bind(id)
    .bind(org_id)
    .bind(rfq_id)
    .bind(currency.code())
    .bind(as_of)
    .bind(&quotation_ids)
    .bind(DbJson(&results))
    .bind(user)
    .fetch_one(&mut **tx)
    .await?;
    Ok(view)
}

/// Supplier names compared as the customer writes them (width, case and spaces ignored).
pub(crate) fn supplier_name_key(name: &str) -> String {
    history::half_width(name)
        .to_lowercase()
        .split_whitespace()
        .collect()
}

/// The decision memo for every result: evidence from the buying criteria, the purchase
/// history (past purchases per supplier) and the comparison itself.
async fn attach_memos(
    tx: &mut db::Tx,
    quotes: &[QuoteRow],
    results: &mut [ComparisonResult],
) -> ApiResult<()> {
    // priorities, preferred, excluded, max lead time.
    type CriteriaRow = (Vec<String>, Vec<String>, Vec<String>, Option<i32>);
    let criteria: Option<CriteriaRow> = sqlx::query_as(
        "SELECT priorities, preferred_suppliers, excluded_suppliers, max_lead_time_days
         FROM buying_criteria",
    )
    .fetch_optional(&mut **tx)
    .await?;
    let (priorities, preferred, excluded, max_lead) = criteria.unwrap_or_default();
    let keys = |v: &[String]| v.iter().map(|n| supplier_name_key(n)).collect::<Vec<_>>();
    let (preferred, excluded) = (keys(&preferred), keys(&excluded));
    let past: Vec<(String, i64)> = sqlx::query_as(
        "SELECT supplier_name, count(*) FROM purchase_history
         WHERE supplier_name <> '' GROUP BY supplier_name",
    )
    .fetch_all(&mut **tx)
    .await?;
    let mut past_by_key: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for (name, n) in past {
        *past_by_key.entry(supplier_name_key(&name)).or_default() +=
            u32::try_from(n).unwrap_or(u32::MAX);
    }
    let candidates: Vec<decision::Candidate> = results
        .iter()
        .map(|r| {
            let key = supplier_name_key(&r.supplier_name);
            decision::Candidate {
                compliant: r.compliance == "compliant",
                total_minor: r.total_minor,
                max_lead_time_days: r.max_lead_time_days,
                domestic: quotes
                    .iter()
                    .find(|q| q.id == r.quotation_id)
                    .map(|q| q.country == "JP"),
                preferred: preferred.contains(&key),
                excluded: excluded.contains(&key),
                past_purchases: past_by_key.get(&key).copied().unwrap_or(0),
                price_alerts: r.price_alerts,
            }
        })
        .collect();
    let assessed = decision::assess(
        &candidates,
        &decision::Criteria {
            priorities,
            max_lead_time_days: max_lead.and_then(|d| u32::try_from(d).ok()),
        },
    );
    for ((r, a), c) in results.iter_mut().zip(assessed).zip(&candidates) {
        r.memo = Some(DecisionMemo {
            score: a.score,
            rank: a.rank,
            factors: a
                .factors
                .iter()
                .map(|f| MemoFactor {
                    factor: f.factor.code().to_string(),
                    weight: f.weight,
                    points: f.points,
                })
                .collect(),
            cautions: a.cautions.iter().map(|c| (*c).to_string()).collect(),
            preferred_supplier: c.preferred,
            past_purchases: c.past_purchases,
        });
    }
    Ok(())
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/rfqs/{rfq_id}/comparisons",
    tag = "rfqs",
    params(("org_id" = Uuid, Path), ("rfq_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Comparison run stored", body = ComparisonView),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown RFQ or not assigned", body = ErrorBody),
        (status = 409, description = "No quotations, or wrong request state", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn run_comparison(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, rfq_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<(StatusCode, Json<ComparisonView>)> {
    let access = require_operator(&p, &state, org_id, Permission::ManageRfqs).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    // Supplier names for the snapshot.
    db::enable_staff_scope(&mut tx).await?;
    let rfq = lock_rfq(&mut tx, rfq_id).await?;
    let req = lock_request(&mut tx, rfq.request_id, View::All).await?;
    let mut view = compare_rfq(&mut tx, org_id, rfq.id, &req, p.user_id).await?;
    transition(
        &mut tx,
        &Audit { org_id, p: &p },
        &req,
        RequestStatus::UnderComparison,
        Side::Operator,
        "comparison.run",
        json!({ "comparison": view.id, "rfq": rfq.number, "quotations": view.results.0.len() }),
    )
    .await?;
    crate::api::ops::close_auto_tasks(&mut tx, req.id, Some(rfq.id), "chase_quotes").await?;
    tx.commit().await?;
    redact_price_checks(access, &mut view.results.0);
    Ok((StatusCode::CREATED, Json(view)))
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/comparisons/{comparison_id}",
    tag = "rfqs",
    params(("org_id" = Uuid, Path), ("comparison_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Stored comparison run", body = ComparisonView),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown comparison", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn get_comparison(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, comparison_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<ComparisonView>> {
    let access = require_quote_reader(&p, &state, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let mut view = sqlx::query_as::<_, ComparisonView>(
        "SELECT id, rfq_id, currency, as_of, results, ran_at FROM comparison_runs WHERE id = $1",
    )
    .bind(comparison_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    tx.commit().await?;
    redact_price_checks(access, &mut view.results.0);
    Ok(Json(view))
}

#[cfg(test)]
mod tests {
    use super::*;
    use procure_domain::rbac::Role;

    fn result_with_check() -> ComparisonResult {
        ComparisonResult {
            quotation_id: Uuid::nil(),
            supplier_id: Uuid::nil(),
            supplier_name: String::new(),
            currency: "JPY".into(),
            compliance: "compliant".into(),
            reasons: vec![],
            total_minor: Some(1),
            max_lead_time_days: None,
            rank: Some(1),
            price_checks: vec![PriceCheck {
                request_item_id: Uuid::nil(),
                quoted_unit_price_minor: 14,
                last_unit_price_milli: 13_000,
                last_currency: "JPY".into(),
                last_purchased_on: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap_or_default(),
                last_supplier_name: "A".into(),
                change_bps: Some(769),
                above_alert: true,
            }],
            price_alerts: 1,
            price_alert_bps: Some(500),
            memo: None,
        }
    }

    #[test]
    fn past_prices_follow_the_history_read_rule() {
        for role in Role::ALL {
            for access in [Access::Member(role), Access::Assigned(role)] {
                let mut rs = [result_with_check()];
                redact_price_checks(access, &mut rs);
                let readable = rbac::allows(access, Permission::ManagePurchaseHistory)
                    || rbac::allows(access, Permission::ViewAllRequests);
                assert_eq!(
                    rs[0].price_checks.len(),
                    usize::from(readable),
                    "{access:?}"
                );
                assert_eq!(rs[0].price_alerts, u32::from(readable), "{access:?}");
                assert_eq!(rs[0].price_alert_bps.is_some(), readable, "{access:?}");
                // Never touches the decision fields.
                assert_eq!(
                    (rs[0].rank, rs[0].compliance.as_str()),
                    (Some(1), "compliant")
                );
            }
        }
        let mut rs = [result_with_check()];
        redact_price_checks(Access::Member(Role::Requester), &mut rs);
        assert!(rs[0].price_checks.is_empty());
    }

    #[test]
    fn runs_stored_before_price_checks_still_load() {
        let old = json!([{
            "quotation_id": Uuid::nil(), "supplier_id": Uuid::nil(), "supplier_name": "S",
            "currency": "JPY", "compliance": "compliant", "reasons": [], "total_minor": 10,
            "max_lead_time_days": 3, "rank": 1
        }]);
        let r: Vec<ComparisonResult> = serde_json::from_value(old).unwrap_or_default();
        assert_eq!(r.len(), 1);
        assert!(r[0].price_checks.is_empty());
        assert_eq!((r[0].price_alerts, r[0].price_alert_bps), (0, None));
    }
}
