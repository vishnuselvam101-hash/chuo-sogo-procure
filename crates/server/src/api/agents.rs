//! Agent console API: settings, runs, the exception inbox and decisions
//! (docs/agents/DESIGN.md). Agents only propose; a decision here is what lets the server
//! act, with the decider's own permissions.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use procure_domain::agents::{Decision, ExceptionStatus, RunStatus, Trigger};
use procure_domain::rbac::{self, Access, Permission};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::AppState;
use crate::agents::exceptions::EXCEPTION_COLUMNS;
use crate::agents::queue::{self, RUN_COLUMNS};
use crate::agents::{self, Actor, AgentException, AgentRun};
use crate::api::admin::AdminPrincipal;
use crate::api::common::{self, Audit};
use crate::api::suppliers::IdResponse;
use crate::db;
use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ApiResult, ErrorBody};
use crate::principal::Principal;

const MAX_BUDGET: i32 = 1_000_000;

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct AgentSettings {
    pub enabled: bool,
    pub auto_send_supplier_mail: bool,
    pub monthly_model_call_budget: i32,
    pub updated_by: Option<Uuid>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct PutAgentSettings {
    pub enabled: bool,
    /// Supplier mail from approved actions is sent only when this is on and the server's
    /// live mail is configured; otherwise it stays a test-mode record.
    pub auto_send_supplier_mail: bool,
    pub monthly_model_call_budget: i32,
}

async fn load_settings(tx: &mut db::Tx, org_id: Uuid) -> ApiResult<AgentSettings> {
    Ok(sqlx::query_as::<_, AgentSettings>(
        "SELECT enabled, auto_send_supplier_mail, monthly_model_call_budget, updated_by,
                updated_at
         FROM agent_settings WHERE org_id = $1",
    )
    .bind(org_id)
    .fetch_optional(&mut **tx)
    .await?
    .unwrap_or(AgentSettings {
        enabled: false,
        auto_send_supplier_mail: false,
        monthly_model_call_budget: 2000,
        updated_by: None,
        updated_at: None,
    }))
}

#[utoipa::path(
    operation_id = "agents_get_settings",
    get,
    path = "/api/v1/orgs/{org_id}/agents/settings",
    tag = "agents",
    params(("org_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Agent settings (defaults when never set: off)", body = AgentSettings),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn get_settings(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
) -> ApiResult<Json<AgentSettings>> {
    p.require(&state, org_id, Permission::ViewAgents).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let s = load_settings(&mut tx, org_id).await?;
    tx.commit().await?;
    Ok(Json(s))
}

#[utoipa::path(
    operation_id = "agents_put_settings",
    put,
    path = "/api/v1/orgs/{org_id}/agents/settings",
    tag = "agents",
    params(("org_id" = Uuid, Path)),
    request_body = PutAgentSettings,
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Settings saved", body = AgentSettings),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 422, description = "Invalid input", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn put_settings(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<PutAgentSettings>,
) -> ApiResult<Json<AgentSettings>> {
    p.require(&state, org_id, Permission::ManageAgents).await?;
    if !(0..=MAX_BUDGET).contains(&body.monthly_model_call_budget) {
        return Err(ApiError::Validation("monthly_model_call_budget"));
    }
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let before = load_settings(&mut tx, org_id).await?;
    sqlx::query(
        "INSERT INTO agent_settings (org_id, enabled, auto_send_supplier_mail,
                                     monthly_model_call_budget, updated_by, updated_at)
         VALUES ($1, $2, $3, $4, $5, now())
         ON CONFLICT (org_id) DO UPDATE SET enabled = EXCLUDED.enabled,
             auto_send_supplier_mail = EXCLUDED.auto_send_supplier_mail,
             monthly_model_call_budget = EXCLUDED.monthly_model_call_budget,
             updated_by = EXCLUDED.updated_by, updated_at = now()",
    )
    .bind(org_id)
    .bind(body.enabled)
    .bind(body.auto_send_supplier_mail)
    .bind(body.monthly_model_call_budget)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "agent.settings_updated",
            "organisation",
            org_id,
            json!({
                "from": {
                    "enabled": before.enabled,
                    "auto_send_supplier_mail": before.auto_send_supplier_mail,
                    "monthly_model_call_budget": before.monthly_model_call_budget,
                },
                "to": {
                    "enabled": body.enabled,
                    "auto_send_supplier_mail": body.auto_send_supplier_mail,
                    "monthly_model_call_budget": body.monthly_model_call_budget,
                },
            }),
        )
        .await?;
    let s = load_settings(&mut tx, org_id).await?;
    tx.commit().await?;
    Ok(Json(s))
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RunsQuery {
    pub agent: Option<String>,
    /// `queued` | `running` | `succeeded` | `failed` | `skipped`
    pub status: Option<String>,
    /// At most 500 (default 100), newest first.
    pub limit: Option<i64>,
}

#[utoipa::path(
    operation_id = "agents_list_runs",
    get,
    path = "/api/v1/orgs/{org_id}/agents/runs",
    tag = "agents",
    params(("org_id" = Uuid, Path), RunsQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Runs, newest first", body = [AgentRun]),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list_runs(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiQuery(q): ApiQuery<RunsQuery>,
) -> ApiResult<Json<Vec<AgentRun>>> {
    p.require(&state, org_id, Permission::ViewAgents).await?;
    if let Some(s) = &q.status
        && RunStatus::from_code(s).is_none()
    {
        return Err(ApiError::BadRequest("status"));
    }
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let runs = sqlx::query_as::<_, AgentRun>(&format!(
        "SELECT {RUN_COLUMNS} FROM agent_runs
         WHERE org_id = $1 AND ($2::text IS NULL OR agent = $2)
           AND ($3::text IS NULL OR status = $3)
         ORDER BY created_at DESC, id DESC LIMIT $4"
    ))
    .bind(org_id)
    .bind(&q.agent)
    .bind(&q.status)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(runs))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct RunRequest {
    pub subject_type: Option<String>,
    pub subject_id: Option<Uuid>,
    #[schema(value_type = Option<Object>)]
    pub input: Option<Value>,
}

#[utoipa::path(
    operation_id = "agents_trigger_run",
    post,
    path = "/api/v1/orgs/{org_id}/agents/{agent}/run",
    tag = "agents",
    params(("org_id" = Uuid, Path), ("agent" = String, Path)),
    request_body = RunRequest,
    security(("bearer" = [])),
    responses(
        (status = 202, description = "Run queued (or the same run already waiting)", body = IdResponse),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation or agent", body = ErrorBody),
        (status = 409, description = "Agents are off for this organisation (agents_disabled)", body = ErrorBody),
        (status = 422, description = "Invalid input", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn trigger_run(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, agent)): ApiPath<(Uuid, String)>,
    ApiJson(body): ApiJson<RunRequest>,
) -> ApiResult<(StatusCode, Json<IdResponse>)> {
    p.require(&state, org_id, Permission::ManageAgents).await?;
    let agent = agents::find(&state, &agent).ok_or(ApiError::NotFound)?;
    let subject = match (&body.subject_type, body.subject_id) {
        (Some(t), Some(id)) => {
            let ok = !t.is_empty()
                && t.len() <= 40
                && t.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
            if !ok {
                return Err(ApiError::Validation("subject_type"));
            }
            Some((t.as_str(), id))
        }
        (None, None) => None,
        _ => return Err(ApiError::Validation("subject")),
    };
    let input = body.input.unwrap_or_else(|| json!({}));
    if !input.is_object() || input.to_string().len() > 32 * 1024 {
        return Err(ApiError::Validation("input"));
    }
    let key = format!(
        "manual:{}:{}",
        agent.name(),
        subject.map(|s| s.1.to_string()).unwrap_or_default()
    );
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let id = queue::enqueue_by(
        &mut tx,
        org_id,
        agent.name(),
        Trigger::Manual,
        subject,
        input,
        Some(&key),
        Some(p.user_id),
    )
    .await?
    .ok_or(ApiError::Conflict("agents_disabled"))?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "agent.run_requested",
            "agent_run",
            id,
            json!({ "agent": agent.name(), "subject_type": subject.map(|s| s.0),
                    "subject_id": subject.map(|s| s.1) }),
        )
        .await?;
    tx.commit().await?;
    Ok((StatusCode::ACCEPTED, Json(IdResponse { id })))
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ExceptionsQuery {
    /// `open` (default) | `approved` | `edited` | `rejected` | `resolved_elsewhere` | `all`
    pub status: Option<String>,
    pub agent: Option<String>,
    /// At most 500 (default 200).
    pub limit: Option<i64>,
}

/// `None` = every status.
fn status_filter(raw: Option<&str>) -> ApiResult<Option<&'static str>> {
    match raw.unwrap_or("open") {
        "all" => Ok(None),
        s => ExceptionStatus::from_code(s)
            .map(|s| Some(s.code()))
            .ok_or(ApiError::BadRequest("status")),
    }
}

async fn exceptions_of(
    tx: &mut db::Tx,
    org_id: Uuid,
    status: Option<&str>,
    agent: Option<&str>,
    limit: i64,
) -> ApiResult<Vec<AgentException>> {
    Ok(sqlx::query_as::<_, AgentException>(&format!(
        "SELECT {EXCEPTION_COLUMNS} FROM agent_exceptions
         WHERE org_id = $1 AND ($2::text IS NULL OR status = $2)
           AND ($3::text IS NULL OR agent = $3)
         ORDER BY CASE severity WHEN 'critical' THEN 0 WHEN 'warning' THEN 1 ELSE 2 END,
                  created_at DESC, id DESC
         LIMIT $4"
    ))
    .bind(org_id)
    .bind(status)
    .bind(agent)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?)
}

#[utoipa::path(
    operation_id = "agents_list_exceptions",
    get,
    path = "/api/v1/orgs/{org_id}/agents/exceptions",
    tag = "agents",
    params(("org_id" = Uuid, Path), ExceptionsQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Exceptions, most severe and newest first", body = [AgentException]),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list_exceptions(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiQuery(q): ApiQuery<ExceptionsQuery>,
) -> ApiResult<Json<Vec<AgentException>>> {
    p.require(&state, org_id, Permission::ViewAgents).await?;
    let status = status_filter(q.status.as_deref())?;
    let limit = q.limit.unwrap_or(200).clamp(1, 500);
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let list = exceptions_of(&mut tx, org_id, status, q.agent.as_deref(), limit).await?;
    tx.commit().await?;
    Ok(Json(list))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct DecideRequest {
    /// `approve` | `edit` | `reject`
    pub decision: String,
    /// For `edit`: the action as changed (same `type` as the proposal).
    #[schema(value_type = Option<Object>)]
    pub action: Option<Value>,
    pub note: Option<String>,
}

#[utoipa::path(
    operation_id = "agents_decide",
    post,
    path = "/api/v1/orgs/{org_id}/agents/exceptions/{exception_id}/decide",
    tag = "agents",
    params(("org_id" = Uuid, Path), ("exception_id" = Uuid, Path)),
    request_body = DecideRequest,
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Decided; for approve/edit the action was executed", body = AgentException),
        (status = 403, description = "Not permitted (to decide, or to perform the action)", body = ErrorBody),
        (status = 404, description = "Unknown organisation or exception", body = ErrorBody),
        (status = 409, description = "Already decided (exception_not_open)", body = ErrorBody),
        (status = 422, description = "Invalid input", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn decide(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, exception_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<DecideRequest>,
) -> ApiResult<Json<AgentException>> {
    let access = p
        .require(&state, org_id, Permission::DecideAgentExceptions)
        .await?;
    let decision = match (body.decision.as_str(), body.action) {
        ("approve", None) => Decision::Approve,
        ("reject", None) => Decision::Reject,
        ("edit", Some(a)) => Decision::Edit(a),
        _ => return Err(ApiError::Validation("decision")),
    };
    let note = common::text(body.note.as_deref().unwrap_or(""), 2000, true, "note")?;
    let actor = Actor {
        principal: &p,
        access,
        state: &state,
    };
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let decided = agents::decide(&mut tx, &actor, org_id, exception_id, decision, &note).await?;
    tx.commit().await?;
    Ok(Json(decided))
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct CountBy {
    pub key: String,
    pub count: i64,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct AgentSummary {
    pub enabled: bool,
    /// Runs of the last 30 days by status.
    pub runs_by_status: Vec<CountBy>,
    /// Open exceptions by severity.
    pub open_exceptions_by_severity: Vec<CountBy>,
    pub model_calls_this_month: i64,
    pub monthly_model_call_budget: i32,
}

#[utoipa::path(
    operation_id = "agents_summary",
    get,
    path = "/api/v1/orgs/{org_id}/agents/summary",
    tag = "agents",
    params(("org_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Counts for the agent console", body = AgentSummary),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn summary(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
) -> ApiResult<Json<AgentSummary>> {
    p.require(&state, org_id, Permission::ViewAgents).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let settings = load_settings(&mut tx, org_id).await?;
    let count = |rows: Vec<(String, i64)>| -> Vec<CountBy> {
        rows.into_iter()
            .map(|(key, count)| CountBy { key, count })
            .collect()
    };
    let runs: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status, count(*) FROM agent_runs
         WHERE org_id = $1 AND created_at > now() - interval '30 days'
         GROUP BY status ORDER BY status",
    )
    .bind(org_id)
    .fetch_all(&mut *tx)
    .await?;
    let open: Vec<(String, i64)> = sqlx::query_as(
        "SELECT severity, count(*) FROM agent_exceptions
         WHERE org_id = $1 AND status = 'open' GROUP BY severity ORDER BY severity",
    )
    .bind(org_id)
    .fetch_all(&mut *tx)
    .await?;
    let calls = queue::model_calls_this_month(&mut tx, org_id).await?;
    tx.commit().await?;
    Ok(Json(AgentSummary {
        enabled: settings.enabled,
        runs_by_status: count(runs),
        open_exceptions_by_severity: count(open),
        model_calls_this_month: calls,
        monthly_model_call_budget: settings.monthly_model_call_budget,
    }))
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct StaffException {
    pub org_name: String,
    pub exception: AgentException,
}

#[utoipa::path(
    operation_id = "admin_agent_exceptions",
    get,
    path = "/api/admin/v1/ops/agents/exceptions",
    tag = "admin",
    params(ExceptionsQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Exceptions of every customer the caller is assigned to", body = [StaffException]),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn staff_exceptions(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiQuery(q): ApiQuery<ExceptionsQuery>,
) -> ApiResult<Json<Vec<StaffException>>> {
    let role = p.platform_role().ok_or(ApiError::Forbidden)?;
    if !rbac::allows(Access::Assigned(role), Permission::ViewAgents) {
        return Err(ApiError::Forbidden);
    }
    let status = status_filter(q.status.as_deref())?;
    let limit = q.limit.unwrap_or(200).clamp(1, 500);
    let mut tx = db::user_tx(&state.pool, p.user_id).await?;
    let orgs: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT o.id, o.name FROM service_assignments a JOIN organisations o ON o.id = a.org_id
         WHERE a.user_id = $1 AND a.revoked_at IS NULL AND o.status = 'active' ORDER BY o.name",
    )
    .bind(p.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let mut out = Vec::new();
    // One transaction per organisation: RLS keeps every query inside that tenant.
    for (org_id, org_name) in orgs {
        let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
        let list = exceptions_of(&mut tx, org_id, status, q.agent.as_deref(), limit).await?;
        tx.commit().await?;
        out.extend(list.into_iter().map(|exception| StaffException {
            org_name: org_name.clone(),
            exception,
        }));
    }
    Ok(Json(out))
}
