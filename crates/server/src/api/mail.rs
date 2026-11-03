//! Our staff's view of supplier mail: whether it is live, and the inbox of messages the
//! server could not match to an RFQ (FAXes, new threads), to read and assign by hand.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use procure_domain::rbac::Permission;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::api::records::Channel;
use crate::api::rfqs::require_operator;
use crate::db;
use crate::error::{ApiError, ApiJson, ApiPath, ApiResult, ErrorBody};
use crate::principal::Principal;

/// Triage of supplier mail that is not yet matched to an organisation: our procurement
/// staff (operators and admins) only.
fn platform_org(p: &Principal) -> ApiResult<Uuid> {
    p.require_platform(Permission::ManageSuppliers)
}

async fn platform_tx(state: &AppState, p: &Principal) -> ApiResult<db::Tx> {
    let mut tx = db::tenant_tx(&state.pool, p.user_id, platform_org(p)?).await?;
    db::enable_platform_scope(&mut tx).await?;
    Ok(tx)
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct MailStatus {
    /// Quote requests are really sent (else only recorded, test mode).
    pub live: bool,
    pub from: Option<String>,
    pub fax: bool,
    /// Replies are read from a mailbox.
    pub inbox: bool,
    /// Email only to these domains (pilot), when not empty.
    pub allow_domains: Vec<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/mail/status",
    tag = "mail",
    security(("bearer" = [])),
    responses(
        (status = 200, description = "How supplier mail is set up on this server", body = MailStatus),
        (status = 403, description = "Not internal staff", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn mail_status(
    State(state): State<AppState>,
    p: Principal,
) -> ApiResult<Json<MailStatus>> {
    platform_org(&p)?;
    let m = state.supplier_mail.as_deref();
    Ok(Json(MailStatus {
        live: m.is_some(),
        from: m.map(|m| m.from.clone()),
        fax: m.is_some_and(|m| m.fax_gateway.is_some()),
        inbox: m.is_some_and(|m| m.inbox.is_some()),
        allow_domains: m.map(|m| m.allow_domains.clone()).unwrap_or_default(),
    }))
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct InboxMessage {
    pub id: Uuid,
    /// email or fax.
    pub channel: String,
    pub from_address: String,
    pub subject: String,
    pub received_at: DateTime<Utc>,
    pub size_bytes: i64,
}

#[utoipa::path(
    get,
    path = "/api/v1/mail-inbox",
    tag = "mail",
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Messages not yet matched to an RFQ, oldest first", body = [InboxMessage]),
        (status = 403, description = "Not internal staff", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list_inbox(
    State(state): State<AppState>,
    p: Principal,
) -> ApiResult<Json<Vec<InboxMessage>>> {
    let mut tx = platform_tx(&state, &p).await?;
    let rows = sqlx::query_as::<_, InboxMessage>(
        "SELECT id, channel, from_address, subject, received_at, size_bytes FROM mail_inbox
         WHERE status = 'unassigned' ORDER BY received_at, id LIMIT 200",
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[derive(sqlx::FromRow)]
struct InboxRow {
    status: String,
    channel: String,
    received_at: DateTime<Utc>,
    sha256: Option<Vec<u8>>,
    org_id: Option<Uuid>,
}

async fn load(state: &AppState, tx: &mut db::Tx, id: Uuid) -> ApiResult<(InboxRow, Vec<u8>)> {
    let row = sqlx::query_as::<_, InboxRow>(
        "SELECT status, channel, received_at, sha256, org_id FROM mail_inbox
         WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    let hash: [u8; 32] = row
        .sha256
        .as_deref()
        .ok_or(ApiError::NotFound)?
        .try_into()
        .map_err(|_| ApiError::internal(std::io::Error::other("bad stored hash")))?;
    let bytes = state.storage.get(&hash).await.map_err(ApiError::internal)?;
    Ok((row, bytes))
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct InboxText {
    pub text: String,
}

#[utoipa::path(
    get,
    path = "/api/v1/mail-inbox/{message_id}/text",
    tag = "mail",
    params(("message_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "The message as text (attachments and FAX pages read too)", body = InboxText),
        (status = 403, description = "Not internal staff", body = ErrorBody),
        (status = 404, description = "Unknown message", body = ErrorBody),
        (status = 422, description = "Unreadable", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn inbox_text(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<Json<InboxText>> {
    let platform = platform_org(&p)?;
    let mut tx = platform_tx(&state, &p).await?;
    let (row, bytes) = load(&state, &mut tx, id).await?;
    // Once filed with an organisation, only staff assigned to it may read it.
    if let Some(org) = row.org_id {
        require_operator(&p, &state, org, Permission::RecordQuotations).await?;
    }
    crate::audit::record(
        &mut tx,
        crate::audit::Event {
            org_id: platform,
            actor: Some(p.user_id),
            action: "mail_inbox.read",
            entity_type: "mail_inbox",
            entity_id: Some(id.to_string()),
            data: json!({}),
            request_id: p.request_id.as_deref(),
        },
    )
    .await?;
    tx.commit().await?;
    let _permit = state
        .upload_permits
        .acquire()
        .await
        .map_err(ApiError::internal)?;
    let text = crate::intake::quote_reply_text("message.eml", &bytes)
        .await
        .map_err(|e| match e {
            crate::intake::IntakeError::Failed => ApiError::Conflict("intake_failed"),
            other => ApiError::Validation(other.code()),
        })?;
    Ok(Json(InboxText { text }))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct AssignBody {
    pub org_id: Uuid,
    /// The RFQ, by id or by its number (`RFQ-2026-000001`).
    pub rfq_id: Option<Uuid>,
    pub rfq_number: Option<String>,
    /// The supplier who sent it (must be asked in the RFQ). When left out, the recipient
    /// whose email domain matches the sender is taken, if exactly one does.
    pub supplier_id: Option<Uuid>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct Assigned {
    pub document_id: Uuid,
}

#[utoipa::path(
    post,
    path = "/api/v1/mail-inbox/{message_id}/assign",
    tag = "mail",
    params(("message_id" = Uuid, Path)),
    request_body = AssignBody,
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Kept as the supplier's document on the RFQ; a task opened", body = Assigned),
        (status = 403, description = "Not staff assigned to that organisation", body = ErrorBody),
        (status = 404, description = "Unknown message or RFQ", body = ErrorBody),
        (status = 409, description = "Already assigned or set aside", body = ErrorBody),
        (status = 422, description = "Supplier not asked in that RFQ", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn assign(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<AssignBody>,
) -> ApiResult<Json<Assigned>> {
    require_operator(&p, &state, body.org_id, Permission::RecordQuotations).await?;
    let from: String = {
        let mut tx = platform_tx(&state, &p).await?;
        let f = sqlx::query_scalar("SELECT from_address FROM mail_inbox WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)?;
        tx.commit().await?;
        f
    };
    // Where it goes: the RFQ in that organisation, and the supplier among its recipients.
    let (rfq_id, request_id, supplier): (Uuid, Uuid, Option<(Uuid, String)>) = {
        let mut tx = db::tenant_tx(&state.pool, p.user_id, body.org_id).await?;
        db::enable_staff_scope(&mut tx).await?;
        let number = body
            .rfq_number
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty());
        let (rfq_id, request_id): (Uuid, Uuid) = sqlx::query_as(
            "SELECT id, request_id FROM rfqs WHERE id = $1 OR (number = $2 AND $1 IS NULL)",
        )
        .bind(body.rfq_id)
        .bind(number)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
        let recipients: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
            "SELECT s.id, s.name, s.email::text FROM rfq_recipients r
             JOIN suppliers s ON s.id = r.supplier_id WHERE r.rfq_id = $1",
        )
        .bind(rfq_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let supplier = match body.supplier_id {
            Some(s) => Some(
                recipients
                    .iter()
                    .find(|r| r.0 == s)
                    .map(|r| (r.0, r.1.clone()))
                    .ok_or(ApiError::Validation("supplier_id"))?,
            ),
            None => {
                let domain = |a: &str| a.rsplit_once('@').map(|(_, d)| d.to_lowercase());
                let sender = domain(&from);
                let hits: Vec<_> = recipients
                    .iter()
                    .filter(|r| sender.is_some() && r.2.as_deref().and_then(domain) == sender)
                    .collect();
                (hits.len() == 1).then(|| (hits[0].0, hits[0].1.clone()))
            }
        };
        (rfq_id, request_id, supplier)
    };
    let platform = platform_org(&p)?;
    // One transaction: the message is locked, kept on the RFQ and marked assigned together.
    let mut tx = platform_tx(&state, &p).await?;
    let (row, bytes) = load(&state, &mut tx, id).await?;
    if row.status != "unassigned" {
        return Err(ApiError::Conflict("already_decided"));
    }
    let channel = if row.channel == "fax" {
        Channel::Fax
    } else {
        Channel::Email
    };
    db::set_org(&mut tx, body.org_id).await?;
    let document_id = crate::supplier_mail::assign_message(
        &state,
        &mut tx,
        &crate::supplier_mail::Incoming {
            org: body.org_id,
            rfq_id,
            request_id,
            supplier: supplier.as_ref().map(|(i, n)| (*i, n.as_str())),
            channel,
            bytes: &bytes,
            received_at: row.received_at,
        },
        Some(&p),
    )
    .await?;
    sqlx::query(
        "UPDATE mail_inbox SET status = 'assigned', org_id = $2, document_id = $3,
                               decided_by = $4, decided_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(body.org_id)
    .bind(document_id)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    crate::audit::record(
        &mut tx,
        crate::audit::Event {
            org_id: platform,
            actor: Some(p.user_id),
            action: "mail_inbox.assigned",
            entity_type: "mail_inbox",
            entity_id: Some(id.to_string()),
            data: json!({ "org": body.org_id, "rfq": rfq_id, "document": document_id }),
            request_id: p.request_id.as_deref(),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Json(Assigned { document_id }))
}

#[utoipa::path(
    post,
    path = "/api/v1/mail-inbox/{message_id}/set-aside",
    tag = "mail",
    params(("message_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Not a supplier reply; kept, no longer listed"),
        (status = 403, description = "Not internal staff", body = ErrorBody),
        (status = 404, description = "Unknown message", body = ErrorBody),
        (status = 409, description = "Already assigned or set aside", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn set_aside(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<StatusCode> {
    let platform = platform_org(&p)?;
    let mut tx = platform_tx(&state, &p).await?;
    let n = sqlx::query(
        "UPDATE mail_inbox SET status = 'set_aside', decided_by = $2, decided_at = now()
         WHERE id = $1 AND status = 'unassigned'",
    )
    .bind(id)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(ApiError::Conflict("already_decided"));
    }
    crate::audit::record(
        &mut tx,
        crate::audit::Event {
            org_id: platform,
            actor: Some(p.user_id),
            action: "mail_inbox.set_aside",
            entity_type: "mail_inbox",
            entity_id: Some(id.to_string()),
            data: json!({}),
            request_id: p.request_id.as_deref(),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
