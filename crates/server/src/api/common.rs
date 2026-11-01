//! Shared pieces of the Phase 2 workflow handlers: input cleaning, request visibility,
//! row locking and audited status transitions.

use procure_domain::money::Currency;
use procure_domain::rbac::{self, Access, Permission};
use procure_domain::workflow::{self, RequestStatus, Side};
use serde_json::Value;
use uuid::Uuid;

use crate::AppState;
use crate::audit::{self, Event};
use crate::db::Tx;
use crate::error::{ApiError, ApiResult};
use crate::principal::Principal;

/// Trimmed text of at most `max` characters, without control characters (newlines and
/// tabs allowed when `multiline`).
pub fn text(raw: &str, max: usize, multiline: bool, field: &'static str) -> ApiResult<String> {
    let t = raw.trim();
    let bad_char = |c: char| c.is_control() && !(multiline && (c == '\n' || c == '\t'));
    if t.chars().count() > max || t.chars().any(bad_char) {
        return Err(ApiError::Validation(field));
    }
    Ok(t.to_string())
}

/// Like `text`, but must not be empty.
pub fn required(raw: &str, max: usize, multiline: bool, field: &'static str) -> ApiResult<String> {
    let t = text(raw, max, multiline, field)?;
    if t.is_empty() {
        return Err(ApiError::Validation(field));
    }
    Ok(t)
}

pub fn currency(code: &str) -> ApiResult<Currency> {
    Currency::from_code(code).ok_or(ApiError::Validation("currency"))
}

/// Which side of the service an access level acts for.
pub fn side(access: Access) -> ApiResult<Side> {
    match access {
        Access::Member(r) if !r.is_internal() => Ok(Side::Customer),
        Access::Assigned(_) => Ok(Side::Operator),
        _ => Err(ApiError::Forbidden),
    }
}

pub fn require_perm(access: Access, perm: Permission) -> ApiResult<()> {
    if rbac::allows(access, perm) {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

/// What part of an organisation's requests the caller may read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    All,
    Own(Uuid),
}

impl View {
    pub fn owner_filter(self) -> Option<Uuid> {
        match self {
            View::All => None,
            View::Own(u) => Some(u),
        }
    }
}

pub async fn request_view(
    p: &Principal,
    state: &AppState,
    org_id: Uuid,
) -> ApiResult<(Access, View)> {
    let access = p.access(state, org_id).await?;
    if access == Access::None {
        return Err(ApiError::NotFound);
    }
    if rbac::allows(access, Permission::ViewAllRequests) {
        Ok((access, View::All))
    } else if rbac::allows(access, Permission::ViewOwnRequests) {
        Ok((access, View::Own(p.user_id)))
    } else {
        Err(ApiError::Forbidden)
    }
}

#[derive(sqlx::FromRow)]
pub struct RequestRow {
    pub id: Uuid,
    pub number: String,
    pub status: String,
    pub currency: String,
    pub requested_by: Uuid,
}

impl RequestRow {
    pub fn status(&self) -> ApiResult<RequestStatus> {
        RequestStatus::from_code(&self.status)
            .ok_or_else(|| ApiError::internal(std::io::Error::other("unknown request status")))
    }

    pub fn currency(&self) -> ApiResult<Currency> {
        Currency::from_code(&self.currency)
            .ok_or_else(|| ApiError::internal(std::io::Error::other("unknown currency")))
    }
}

/// Locks the request row for the rest of the transaction. Requests outside `view`
/// do not exist for the caller.
pub async fn lock_request(tx: &mut Tx, id: Uuid, view: View) -> ApiResult<RequestRow> {
    sqlx::query_as::<_, RequestRow>(
        "SELECT id, number, status, currency, requested_by FROM procurement_requests
         WHERE id = $1 AND ($2::uuid IS NULL OR requested_by = $2) FOR UPDATE",
    )
    .bind(id)
    .bind(view.owner_filter())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)
}

pub fn invalid_transition(_: workflow::InvalidTransition) -> ApiError {
    ApiError::Conflict("invalid_transition")
}

pub struct Audit<'a> {
    pub org_id: Uuid,
    pub p: &'a Principal,
}

impl Audit<'_> {
    pub async fn record(
        &self,
        tx: &mut Tx,
        action: &str,
        entity_type: &str,
        entity_id: Uuid,
        data: Value,
    ) -> ApiResult<()> {
        audit::record(
            tx,
            Event {
                org_id: self.org_id,
                actor: Some(self.p.user_id),
                action,
                entity_type,
                entity_id: Some(entity_id.to_string()),
                data,
                request_id: self.p.request_id.as_deref(),
            },
        )
        .await?;
        Ok(())
    }
}

/// Moves a locked request to `to` (checked against the workflow table for `side`) and
/// records `action` with the old and new status in the same transaction.
pub async fn transition(
    tx: &mut Tx,
    audit: &Audit<'_>,
    req: &RequestRow,
    to: RequestStatus,
    side: Side,
    action: &str,
    mut data: Value,
) -> ApiResult<()> {
    let from = req.status()?;
    workflow::check(from, to, side).map_err(invalid_transition)?;
    sqlx::query(
        "UPDATE procurement_requests SET status = $2, updated_at = now(),
                submitted_at = CASE WHEN $2 = 'submitted' THEN now() ELSE submitted_at END
         WHERE id = $1",
    )
    .bind(req.id)
    .bind(to.code())
    .execute(&mut **tx)
    .await?;
    // Out of Chuo-Sogo's hands: open automatic tasks (and their deadlines) stop.
    if matches!(
        to,
        RequestStatus::Cancelled | RequestStatus::Rejected | RequestStatus::NeedsClarification
    ) {
        crate::api::ops::cancel_auto_tasks(tx, req.id).await?;
    }
    if let Value::Object(m) = &mut data {
        m.insert("from".into(), from.code().into());
        m.insert("to".into(), to.code().into());
        m.insert("number".into(), req.number.clone().into());
    }
    audit
        .record(tx, action, "procurement_request", req.id, data)
        .await
}

/// Advances the request only if it is currently in `from` (used for side effects such as
/// "first quotation recorded" that may happen repeatedly).
pub async fn advance_if(
    tx: &mut Tx,
    audit: &Audit<'_>,
    req: &RequestRow,
    from: RequestStatus,
    to: RequestStatus,
    action: &str,
) -> ApiResult<()> {
    if req.status()? == from {
        transition(
            tx,
            audit,
            req,
            to,
            Side::Operator,
            action,
            Value::Object(Default::default()),
        )
        .await?;
    }
    Ok(())
}
