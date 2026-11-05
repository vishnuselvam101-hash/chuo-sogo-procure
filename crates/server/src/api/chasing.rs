//! Chasing agent settings (lane L3): the organisation's follow-up cadence in Japan business
//! days. Viewing needs `ViewAgents`; changing it `ManageAgents`.

use axum::Json;
use axum::extract::State;
use chrono::{DateTime, Utc};
use procure_domain::chasing::Cadence;
use procure_domain::rbac::Permission;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::agents::chasing::{load_settings, save_settings};
use crate::api::common::Audit;
use crate::db;
use crate::error::{ApiError, ApiJson, ApiPath, ApiResult, ErrorBody};
use crate::principal::Principal;

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct ChaseCadence {
    /// Business days after the order email before asking for an acknowledgement (1–30).
    pub ack_days: i32,
    /// Business days after the order email before asking for a delivery date (1–30).
    pub delivery_date_days: i32,
    /// Business days after the expected delivery date before asking where the goods are (1–30).
    pub overdue_days: i32,
    /// Business days after everything arrived before asking for the invoice (1–60).
    pub invoice_days: i32,
    /// Business days between follow-ups (1–30).
    pub repeat_days: i32,
    /// Follow-ups per order and type before a person takes over (1–10).
    pub max_attempts: i32,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ChaseSettings {
    #[serde(flatten)]
    pub cadence: ChaseCadence,
    pub updated_by: Option<Uuid>,
    pub updated_at: Option<DateTime<Utc>>,
}

fn to_view(c: Cadence) -> ChaseCadence {
    ChaseCadence {
        ack_days: c.ack_days,
        delivery_date_days: c.delivery_date_days,
        overdue_days: c.overdue_days,
        invoice_days: c.invoice_days,
        repeat_days: c.repeat_days,
        max_attempts: c.max_attempts,
    }
}

fn from_view(c: &ChaseCadence) -> Cadence {
    Cadence {
        ack_days: c.ack_days,
        delivery_date_days: c.delivery_date_days,
        overdue_days: c.overdue_days,
        invoice_days: c.invoice_days,
        repeat_days: c.repeat_days,
        max_attempts: c.max_attempts,
    }
}

async fn view(tx: &mut db::Tx, org_id: Uuid) -> ApiResult<ChaseSettings> {
    let (c, updated_by, updated_at) = load_settings(tx, org_id).await?;
    Ok(ChaseSettings {
        cadence: to_view(c),
        updated_by,
        updated_at,
    })
}

#[utoipa::path(
    operation_id = "chasing_get_settings",
    get,
    path = "/api/v1/orgs/{org_id}/agents/chasing/settings",
    tag = "agents",
    params(("org_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Follow-up cadence (defaults when never set)", body = ChaseSettings),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn get_settings(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
) -> ApiResult<Json<ChaseSettings>> {
    p.require(&state, org_id, Permission::ViewAgents).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let s = view(&mut tx, org_id).await?;
    tx.commit().await?;
    Ok(Json(s))
}

#[utoipa::path(
    operation_id = "chasing_put_settings",
    put,
    path = "/api/v1/orgs/{org_id}/agents/chasing/settings",
    tag = "agents",
    params(("org_id" = Uuid, Path)),
    request_body = ChaseCadence,
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Cadence saved", body = ChaseSettings),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 422, description = "A value out of range", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn put_settings(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<ChaseCadence>,
) -> ApiResult<Json<ChaseSettings>> {
    p.require(&state, org_id, Permission::ManageAgents).await?;
    let c = from_view(&body);
    c.validate().map_err(ApiError::Validation)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let before = view(&mut tx, org_id).await?;
    save_settings(&mut tx, org_id, &c, p.user_id).await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "chasing.settings_updated",
            "organisation",
            org_id,
            json!({ "from": before.cadence, "to": body }),
        )
        .await?;
    let s = view(&mut tx, org_id).await?;
    tx.commit().await?;
    Ok(Json(s))
}
