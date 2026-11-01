//! Managed operations: staff work queue (tasks with due dates), automatic tasks on workflow
//! steps, follow-ups for unanswered RFQs, request comments (customer-visible or internal),
//! supplier communications log and service time.
//!
//! Internal records are audited in Chuo-Sogo's own (platform) audit log: customer admins
//! can read their organisation's log, and internal notes must never reach them.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use procure_domain::rbac::{self, Access, Permission};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::api::common::{self, Audit, request_view};
use crate::api::suppliers::IdResponse;
use crate::db;
use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ApiResult, ErrorBody};
use crate::principal::Principal;

pub const TASK_KINDS: [&str; 7] = [
    "identify",
    "source",
    "chase_quotes",
    "compare",
    "follow_up",
    "exception",
    "other",
];
const TASK_STATUSES: [&str; 4] = ["open", "in_progress", "done", "cancelled"];
const PRIORITIES: [&str; 4] = ["low", "normal", "high", "urgent"];

/// Service level for identifying products after a request is submitted.
pub const IDENTIFY_SLA: Duration = Duration::hours(24);
/// Chasing quotations when the RFQ has no response date.
pub const CHASE_DEFAULT: Duration = Duration::hours(72);

// ---------------------------------------------------------------------------------------
// Automatic tasks, called from workflow handlers inside their transactions.

/// Runs `f` with staff scope on and restores the previous value: automatic tasks are
/// created inside customer-side transactions (e.g. a requester submitting).
async fn with_staff_scope(tx: &mut db::Tx) -> ApiResult<String> {
    let prev: String =
        sqlx::query_scalar("SELECT coalesce(current_setting('app.staff', true), '')")
            .fetch_one(&mut **tx)
            .await?;
    db::enable_staff_scope(tx).await?;
    Ok(prev)
}

async fn restore_scope(tx: &mut db::Tx, prev: &str) -> ApiResult<()> {
    sqlx::query("SELECT set_config('app.staff', $1, true)")
        .bind(prev)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// End of `date` in the organisation's own timezone (due dates are local business days).
pub async fn end_of_local_day(
    tx: &mut db::Tx,
    org_id: Uuid,
    date: NaiveDate,
) -> ApiResult<DateTime<Utc>> {
    Ok(sqlx::query_scalar(
        "SELECT (($2::date + 1)::timestamp AT TIME ZONE timezone) - interval '1 second'
         FROM organisations WHERE id = $1",
    )
    .bind(org_id)
    .bind(date)
    .fetch_one(&mut **tx)
    .await?)
}

/// Opens an automatic task (created_by NULL) unless one is already open: one `identify`
/// per request, one `chase_quotes` per RFQ.
pub async fn open_auto_task(
    tx: &mut db::Tx,
    org_id: Uuid,
    request_id: Uuid,
    rfq_id: Option<Uuid>,
    kind: &str,
    title: &str,
    due_at: DateTime<Utc>,
) -> ApiResult<()> {
    let prev = with_staff_scope(tx).await?;
    sqlx::query(
        "INSERT INTO tasks (id, org_id, request_id, rfq_id, kind, title, due_at)
         SELECT $1, $2, $3, $4, $5, $6, $7
         WHERE NOT EXISTS (SELECT 1 FROM tasks
                           WHERE request_id = $3 AND kind = $5 AND created_by IS NULL
                             AND rfq_id IS NOT DISTINCT FROM $4
                             AND status IN ('open', 'in_progress'))",
    )
    .bind(Uuid::now_v7())
    .bind(org_id)
    .bind(request_id)
    .bind(rfq_id)
    .bind(kind)
    .bind(title)
    .bind(due_at)
    .execute(&mut **tx)
    .await?;
    restore_scope(tx, &prev).await
}

/// Completes open automatic tasks of `kind` for the request (or only for one RFQ).
/// Manual tasks are never closed automatically.
pub async fn close_auto_tasks(
    tx: &mut db::Tx,
    request_id: Uuid,
    rfq_id: Option<Uuid>,
    kind: &str,
) -> ApiResult<()> {
    let prev = with_staff_scope(tx).await?;
    sqlx::query(
        "UPDATE tasks SET status = 'done', completed_at = now()
         WHERE request_id = $1 AND kind = $2 AND created_by IS NULL
           AND ($3::uuid IS NULL OR rfq_id = $3) AND status IN ('open', 'in_progress')",
    )
    .bind(request_id)
    .bind(kind)
    .bind(rfq_id)
    .execute(&mut **tx)
    .await?;
    restore_scope(tx, &prev).await
}

/// The request left Chuo-Sogo's hands (cancelled, rejected, waiting for the customer's
/// answer): its open automatic tasks stop counting. A resubmission opens fresh ones.
pub async fn cancel_auto_tasks(tx: &mut db::Tx, request_id: Uuid) -> ApiResult<()> {
    let prev = with_staff_scope(tx).await?;
    sqlx::query(
        "UPDATE tasks SET status = 'cancelled', completed_at = now()
         WHERE request_id = $1 AND created_by IS NULL AND status IN ('open', 'in_progress')",
    )
    .bind(request_id)
    .execute(&mut **tx)
    .await?;
    restore_scope(tx, &prev).await
}

// ---------------------------------------------------------------------------------------
// Access helpers.

/// Assigned staff with `perm` on the customer org; returns the caller's platform org id
/// (for internal audit) after enabling staff scope on `tx`.
async fn require_staff(
    p: &Principal,
    state: &AppState,
    org_id: Uuid,
    perm: Permission,
) -> ApiResult<Uuid> {
    let access = p.require(state, org_id, perm).await?;
    if !matches!(access, Access::Assigned(_)) {
        return Err(ApiError::Forbidden);
    }
    platform_org(p)
}

fn platform_org(p: &Principal) -> ApiResult<Uuid> {
    p.memberships
        .iter()
        .find(|m| m.role.is_internal())
        .map(|m| m.org_id)
        .ok_or(ApiError::Forbidden)
}

/// Internal events are recorded in the platform (Chuo-Sogo) audit log, then the
/// transaction is switched back to the customer org.
struct InternalAudit<'a> {
    p: &'a Principal,
    platform: Uuid,
    customer: Uuid,
}

impl InternalAudit<'_> {
    async fn record(
        &self,
        tx: &mut db::Tx,
        action: &str,
        entity_type: &str,
        entity_id: Uuid,
        mut data: serde_json::Value,
    ) -> ApiResult<()> {
        if let serde_json::Value::Object(m) = &mut data {
            m.insert("customer_org".into(), self.customer.to_string().into());
        }
        Audit {
            org_id: self.platform,
            p: self.p,
        }
        .record(tx, action, entity_type, entity_id, data)
        .await?;
        db::set_org(tx, self.customer).await?;
        Ok(())
    }
}

async fn request_exists(tx: &mut db::Tx, request_id: Uuid) -> ApiResult<()> {
    let ok: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM procurement_requests WHERE id = $1)")
            .bind(request_id)
            .fetch_one(&mut **tx)
            .await?;
    if ok { Ok(()) } else { Err(ApiError::NotFound) }
}

/// Staff that may hold a task in this org: platform members with an active assignment.
async fn assignable(tx: &mut db::Tx, org_id: Uuid, user_id: Uuid) -> ApiResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM service_assignments
                        WHERE org_id = $1 AND user_id = $2 AND revoked_at IS NULL)",
    )
    .bind(org_id)
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?)
}

// ---------------------------------------------------------------------------------------
// Tasks.

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct TaskView {
    pub id: Uuid,
    pub org_id: Uuid,
    pub request_id: Option<Uuid>,
    pub request_number: Option<String>,
    pub kind: String,
    pub title: String,
    pub status: String,
    pub priority: String,
    pub assignee_id: Option<Uuid>,
    pub assignee_name: Option<String>,
    pub due_at: Option<DateTime<Utc>>,
    /// Open and past its due time.
    pub overdue: bool,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

const TASK_SELECT: &str = "SELECT t.id, t.org_id, t.request_id, r.number AS request_number, t.kind,
        t.title, t.status, t.priority, t.assignee_id, u.display_name AS assignee_name, t.due_at,
        (t.status IN ('open', 'in_progress') AND t.due_at IS NOT NULL AND t.due_at < now()) AS overdue,
        t.created_at, t.completed_at
    FROM tasks t
    LEFT JOIN procurement_requests r ON r.id = t.request_id
    LEFT JOIN users u ON u.id = t.assignee_id";

#[derive(Deserialize, utoipa::ToSchema)]
pub struct TaskInput {
    pub request_id: Option<Uuid>,
    pub kind: String,
    pub title: String,
    #[serde(default = "normal")]
    pub priority: String,
    pub due_at: Option<DateTime<Utc>>,
    /// Alternative to `due_at`: end of this day in the customer's timezone.
    pub due_date: Option<NaiveDate>,
    pub assignee_id: Option<Uuid>,
}

fn normal() -> String {
    "normal".into()
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/tasks",
    tag = "operations",
    params(("org_id" = Uuid, Path)),
    request_body = TaskInput,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Task created", body = IdResponse),
        (status = 403, description = "Not assigned staff, or may not assign others", body = ErrorBody),
        (status = 404, description = "Unknown request or organisation", body = ErrorBody),
        (status = 422, description = "Invalid input or assignee", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn create_task(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<TaskInput>,
) -> ApiResult<(StatusCode, Json<IdResponse>)> {
    let platform = require_staff(&p, &state, org_id, Permission::OperateRequests).await?;
    if !TASK_KINDS.contains(&body.kind.as_str()) {
        return Err(ApiError::Validation("kind"));
    }
    if !PRIORITIES.contains(&body.priority.as_str()) {
        return Err(ApiError::Validation("priority"));
    }
    let title = common::required(&body.title, 200, false, "title")?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    db::enable_staff_scope(&mut tx).await?;
    if let Some(r) = body.request_id {
        request_exists(&mut tx, r).await?;
    }
    let due_at = match (body.due_at, body.due_date) {
        (Some(_), Some(_)) => return Err(ApiError::Validation("due_at")),
        (Some(d), None) => Some(d),
        (None, Some(d)) => Some(end_of_local_day(&mut tx, org_id, d).await?),
        (None, None) => None,
    };
    if due_at.is_some_and(|d| d > Utc::now() + Duration::days(730)) {
        return Err(ApiError::Validation("due_at"));
    }
    if let Some(a) = body.assignee_id {
        if a != p.user_id && !rbac::allows(p.access(&state, org_id).await?, Permission::AssignWork)
        {
            return Err(ApiError::Forbidden);
        }
        if !assignable(&mut tx, org_id, a).await? {
            return Err(ApiError::Validation("assignee"));
        }
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tasks (id, org_id, request_id, kind, title, priority, due_at, assignee_id, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(org_id)
    .bind(body.request_id)
    .bind(&body.kind)
    .bind(&title)
    .bind(&body.priority)
    .bind(due_at)
    .bind(body.assignee_id)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    InternalAudit {
        p: &p,
        platform,
        customer: org_id,
    }
    .record(
        &mut tx,
        "task.created",
        "task",
        id,
        json!({ "kind": body.kind, "request": body.request_id, "assignee": body.assignee_id }),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(IdResponse { id })))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct TaskUpdate {
    pub status: Option<String>,
    pub priority: Option<String>,
    /// Use `assign_to_me` to take a task; assigning someone else needs AssignWork.
    pub assignee_id: Option<Uuid>,
    #[serde(default)]
    pub unassign: bool,
    pub due_at: Option<DateTime<Utc>>,
}

#[utoipa::path(
    patch,
    path = "/api/v1/orgs/{org_id}/tasks/{task_id}",
    tag = "operations",
    params(("org_id" = Uuid, Path), ("task_id" = Uuid, Path)),
    request_body = TaskUpdate,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Updated"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown task", body = ErrorBody),
        (status = 409, description = "Task already closed", body = ErrorBody),
        (status = 422, description = "Invalid input or assignee", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn update_task(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, task_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<TaskUpdate>,
) -> ApiResult<StatusCode> {
    let platform = require_staff(&p, &state, org_id, Permission::OperateRequests).await?;
    if let Some(s) = &body.status
        && !TASK_STATUSES.contains(&s.as_str())
    {
        return Err(ApiError::Validation("status"));
    }
    if let Some(pr) = &body.priority
        && !PRIORITIES.contains(&pr.as_str())
    {
        return Err(ApiError::Validation("priority"));
    }
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    db::enable_staff_scope(&mut tx).await?;
    let (current, holder): (String, Option<Uuid>) =
        sqlx::query_as("SELECT status, assignee_id FROM tasks WHERE id = $1 FOR UPDATE")
            .bind(task_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    if matches!(current.as_str(), "done" | "cancelled") {
        return Err(ApiError::Conflict("task_closed"));
    }
    if body.unassign && body.assignee_id.is_some() {
        return Err(ApiError::Validation("assignee"));
    }
    // Taking over or dropping someone else's task is an assignment decision.
    let changes_holder = body.unassign || body.assignee_id.is_some();
    let holder_is_active = match holder {
        Some(h) => assignable(&mut tx, org_id, h).await?,
        None => false,
    };
    if changes_holder
        && holder.is_some_and(|h| h != p.user_id)
        && holder_is_active
        && !rbac::allows(p.access(&state, org_id).await?, Permission::AssignWork)
    {
        return Err(ApiError::Forbidden);
    }
    if let Some(a) = body.assignee_id {
        if a != p.user_id && !rbac::allows(p.access(&state, org_id).await?, Permission::AssignWork)
        {
            return Err(ApiError::Forbidden);
        }
        if !assignable(&mut tx, org_id, a).await? {
            return Err(ApiError::Validation("assignee"));
        }
    }
    sqlx::query(
        "UPDATE tasks SET
             status = coalesce($2, status),
             priority = coalesce($3, priority),
             assignee_id = CASE WHEN $5 THEN NULL ELSE coalesce($4, assignee_id) END,
             due_at = coalesce($6, due_at),
             completed_at = CASE WHEN coalesce($2, status) IN ('done', 'cancelled') THEN now() ELSE NULL END
         WHERE id = $1",
    )
    .bind(task_id)
    .bind(&body.status)
    .bind(&body.priority)
    .bind(body.assignee_id)
    .bind(body.unassign)
    .bind(body.due_at)
    .execute(&mut *tx)
    .await?;
    InternalAudit {
        p: &p,
        platform,
        customer: org_id,
    }
    .record(
        &mut tx,
        "task.updated",
        "task",
        task_id,
        json!({ "status": body.status, "priority": body.priority, "assignee": body.assignee_id,
                "unassign": body.unassign, "due_at": body.due_at }),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/tasks",
    tag = "operations",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Tasks of the request (staff)", body = [TaskView]),
        (status = 403, description = "Not assigned staff", body = ErrorBody),
        (status = 404, description = "Unknown request", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn request_tasks(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<Vec<TaskView>>> {
    require_staff(&p, &state, org_id, Permission::OperateRequests).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    db::enable_staff_scope(&mut tx).await?;
    request_exists(&mut tx, request_id).await?;
    let rows = sqlx::query_as::<_, TaskView>(&format!(
        "{TASK_SELECT} WHERE t.request_id = $1 ORDER BY t.status IN ('done', 'cancelled'), t.due_at NULLS LAST, t.created_at"
    ))
    .bind(request_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

// ---------------------------------------------------------------------------------------
// The staff work queue across all customer organisations the caller is assigned to.

#[derive(Serialize, utoipa::ToSchema)]
pub struct QueueTask {
    pub org_name: String,
    /// The customer's timezone, for showing due times as the customer counts them.
    pub org_timezone: String,
    #[serde(flatten)]
    pub task: TaskView,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct FollowUp {
    pub org_id: Uuid,
    pub org_name: String,
    pub request_id: Uuid,
    pub request_number: String,
    pub rfq_id: Uuid,
    pub rfq_number: String,
    pub response_due: NaiveDate,
    /// Suppliers that have not answered yet.
    pub waiting_on: Vec<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct Queue {
    /// Open tasks, overdue first, then by due time.
    pub tasks: Vec<QueueTask>,
    /// Sent RFQs past their response date with suppliers still silent.
    pub follow_ups: Vec<FollowUp>,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct QueueQuery {
    /// Only tasks assigned to me (default: mine and unassigned).
    #[serde(default)]
    pub mine: bool,
}

#[utoipa::path(
    get,
    path = "/api/v1/ops/queue",
    tag = "operations",
    params(QueueQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Work queue across assigned customer organisations", body = Queue),
        (status = 403, description = "Not internal staff", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn queue(
    State(state): State<AppState>,
    p: Principal,
    ApiQuery(q): ApiQuery<QueueQuery>,
) -> ApiResult<Json<Queue>> {
    // Same rule as every other operations endpoint: staff whose role may operate requests
    // on assigned customers (not, e.g., the platform super administrator).
    let role = p.platform_role().ok_or(ApiError::Forbidden)?;
    if !rbac::allows(Access::Assigned(role), Permission::OperateRequests) {
        return Err(ApiError::Forbidden);
    }
    let mut tx = db::user_tx(&state.pool, p.user_id).await?;
    let orgs: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT o.id, o.name, o.timezone FROM service_assignments a JOIN organisations o ON o.id = a.org_id
         WHERE a.user_id = $1 AND a.revoked_at IS NULL AND o.status = 'active' ORDER BY o.name",
    )
    .bind(p.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let mut tasks = Vec::new();
    let mut follow_ups = Vec::new();
    // One transaction per organisation: RLS keeps every query inside that tenant.
    for (org_id, org_name, org_timezone) in orgs {
        let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
        db::enable_staff_scope(&mut tx).await?;
        let rows = sqlx::query_as::<_, TaskView>(&format!(
            "{TASK_SELECT} WHERE t.status IN ('open', 'in_progress')
                AND (t.assignee_id = $1 OR (NOT $2 AND (
                    t.assignee_id IS NULL
                    -- Held by someone no longer assigned to this customer: shown as unassigned.
                    OR NOT EXISTS (SELECT 1 FROM service_assignments a
                                   WHERE a.org_id = t.org_id AND a.user_id = t.assignee_id
                                     AND a.revoked_at IS NULL))))"
        ))
        .bind(p.user_id)
        .bind(q.mine)
        .fetch_all(&mut *tx)
        .await?;
        tasks.extend(rows.into_iter().map(|task| QueueTask {
            org_name: org_name.clone(),
            org_timezone: org_timezone.clone(),
            task,
        }));
        let due = sqlx::query_as::<_, FollowUp>(
            "SELECT f.org_id, $1::text AS org_name, r.id AS request_id, r.number AS request_number,
                    f.id AS rfq_id, f.number AS rfq_number, f.response_due,
                    array_agg(s.name ORDER BY s.name) AS waiting_on
             FROM rfqs f
             JOIN procurement_requests r ON r.id = f.request_id
             JOIN rfq_recipients x ON x.rfq_id = f.id AND x.status = 'exported'
             JOIN suppliers s ON s.id = x.supplier_id
             JOIN organisations o ON o.id = f.org_id
             WHERE f.status = 'sent' AND f.response_due IS NOT NULL
               AND f.response_due < (now() AT TIME ZONE o.timezone)::date
               AND r.status IN ('rfq_sent', 'quotes_received', 'under_comparison')
             GROUP BY f.org_id, r.id, r.number, f.id, f.number, f.response_due
             ORDER BY f.response_due",
        )
        .bind(&org_name)
        .fetch_all(&mut *tx)
        .await?;
        follow_ups.extend(due);
        tx.commit().await?;
    }
    tasks.sort_by(|a, b| {
        b.task
            .overdue
            .cmp(&a.task.overdue)
            .then(a.task.due_at.is_none().cmp(&b.task.due_at.is_none()))
            .then(a.task.due_at.cmp(&b.task.due_at))
            .then(a.task.created_at.cmp(&b.task.created_at))
    });
    Ok(Json(Queue { tasks, follow_ups }))
}

// ---------------------------------------------------------------------------------------
// Comments.

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct CommentView {
    pub id: Uuid,
    pub author_id: Uuid,
    pub author_name: Option<String>,
    /// `customer` (shared) or `internal` (Chuo-Sogo only).
    pub visibility: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct CommentInput {
    pub body: String,
    /// `customer` (default) or `internal` (staff only).
    #[serde(default = "customer")]
    pub visibility: String,
}

fn customer() -> String {
    "customer".into()
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/comments",
    tag = "operations",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Comments visible to the caller, oldest first", body = [CommentView]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or not visible", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list_comments(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<Vec<CommentView>>> {
    let (access, view) = request_view(&p, &state, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    if rbac::allows(access, Permission::ViewInternalNotes) {
        db::enable_staff_scope(&mut tx).await?;
    }
    common::lock_request(&mut tx, request_id, view).await?;
    // Customers' transactions never have staff scope: RLS hides internal comments from them.
    let rows = sqlx::query_as::<_, CommentView>(
        "SELECT c.id, c.author_id, u.display_name AS author_name, c.visibility, c.body, c.created_at
         FROM comments c LEFT JOIN users u ON u.id = c.author_id
         WHERE c.request_id = $1 ORDER BY c.created_at, c.id",
    )
    .bind(request_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/comments",
    tag = "operations",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    request_body = CommentInput,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Comment added", body = IdResponse),
        (status = 403, description = "Internal notes are for Chuo-Sogo staff only", body = ErrorBody),
        (status = 404, description = "Unknown or not visible", body = ErrorBody),
        (status = 422, description = "Empty or too long", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn add_comment(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<CommentInput>,
) -> ApiResult<(StatusCode, Json<IdResponse>)> {
    let (access, view) = request_view(&p, &state, org_id).await?;
    let internal = match body.visibility.as_str() {
        "customer" => false,
        "internal" => true,
        _ => return Err(ApiError::Validation("visibility")),
    };
    if internal && !rbac::allows(access, Permission::ViewInternalNotes) {
        return Err(ApiError::Forbidden);
    }
    let text = common::required(&body.body, 4000, true, "body")?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let req = common::lock_request(&mut tx, request_id, view).await?;
    if internal {
        db::enable_staff_scope(&mut tx).await?;
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO comments (id, org_id, request_id, author_id, visibility, body)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(org_id)
    .bind(req.id)
    .bind(p.user_id)
    .bind(&body.visibility)
    .bind(&text)
    .execute(&mut *tx)
    .await?;
    // The text itself stays out of the audit log; internal notes are logged with Chuo-Sogo.
    let data = json!({ "request": req.id, "number": req.number, "visibility": body.visibility });
    if internal {
        InternalAudit {
            p: &p,
            platform: platform_org(&p)?,
            customer: org_id,
        }
        .record(&mut tx, "comment.added", "comment", id, data)
        .await?;
    } else {
        Audit { org_id, p: &p }
            .record(&mut tx, "comment.added", "comment", id, data)
            .await?;
    }
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(IdResponse { id })))
}

// ---------------------------------------------------------------------------------------
// Supplier communications and service time (internal).

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct CommunicationView {
    pub id: Uuid,
    pub supplier_id: Option<Uuid>,
    pub supplier_name: Option<String>,
    pub direction: String,
    pub channel: String,
    pub summary: String,
    pub occurred_at: DateTime<Utc>,
    pub recorded_by_name: Option<String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct CommunicationInput {
    pub supplier_id: Option<Uuid>,
    /// `inbound` or `outbound`.
    pub direction: String,
    /// `email`, `phone`, `meeting`, `chat` or `other`.
    pub channel: String,
    pub summary: String,
    pub occurred_at: Option<DateTime<Utc>>,
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/communications",
    tag = "operations",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Supplier communications (staff)", body = [CommunicationView]),
        (status = 403, description = "Not assigned staff", body = ErrorBody),
        (status = 404, description = "Unknown request", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list_communications(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<Vec<CommunicationView>>> {
    require_staff(&p, &state, org_id, Permission::OperateRequests).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    db::enable_staff_scope(&mut tx).await?;
    request_exists(&mut tx, request_id).await?;
    let rows = sqlx::query_as::<_, CommunicationView>(
        "SELECT c.id, c.supplier_id, s.name AS supplier_name, c.direction, c.channel, c.summary,
                c.occurred_at, u.display_name AS recorded_by_name
         FROM communications c LEFT JOIN suppliers s ON s.id = c.supplier_id
         LEFT JOIN users u ON u.id = c.recorded_by
         WHERE c.request_id = $1 ORDER BY c.occurred_at DESC, c.id",
    )
    .bind(request_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/communications",
    tag = "operations",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    request_body = CommunicationInput,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Communication logged", body = IdResponse),
        (status = 403, description = "Not assigned staff", body = ErrorBody),
        (status = 404, description = "Unknown request", body = ErrorBody),
        (status = 422, description = "Invalid input", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn log_communication(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<CommunicationInput>,
) -> ApiResult<(StatusCode, Json<IdResponse>)> {
    let platform = require_staff(&p, &state, org_id, Permission::OperateRequests).await?;
    if !["inbound", "outbound"].contains(&body.direction.as_str()) {
        return Err(ApiError::Validation("direction"));
    }
    if !["email", "phone", "meeting", "chat", "other"].contains(&body.channel.as_str()) {
        return Err(ApiError::Validation("channel"));
    }
    let summary = common::required(&body.summary, 4000, true, "summary")?;
    let occurred_at = body.occurred_at.unwrap_or_else(Utc::now);
    if occurred_at > Utc::now() + Duration::minutes(5) {
        return Err(ApiError::Validation("occurred_at"));
    }
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    db::enable_staff_scope(&mut tx).await?;
    request_exists(&mut tx, request_id).await?;
    if let Some(s) = body.supplier_id {
        let known: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM suppliers WHERE id = $1)")
                .bind(s)
                .fetch_one(&mut *tx)
                .await?;
        if !known {
            return Err(ApiError::Validation("supplier_id"));
        }
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO communications (id, org_id, request_id, supplier_id, direction, channel,
                                     summary, occurred_at, recorded_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(org_id)
    .bind(request_id)
    .bind(body.supplier_id)
    .bind(&body.direction)
    .bind(&body.channel)
    .bind(&summary)
    .bind(occurred_at)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    InternalAudit {
        p: &p,
        platform,
        customer: org_id,
    }
    .record(
        &mut tx,
        "communication.logged",
        "communication",
        id,
        json!({ "request": request_id, "supplier": body.supplier_id, "channel": body.channel,
                "direction": body.direction }),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(IdResponse { id })))
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct TimeEntryView {
    pub id: Uuid,
    pub user_id: Uuid,
    pub user_name: Option<String>,
    pub task_id: Option<Uuid>,
    pub work_date: NaiveDate,
    pub minutes: i32,
    pub note: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct TimeSheet {
    pub entries: Vec<TimeEntryView>,
    pub total_minutes: i64,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct TimeInput {
    pub minutes: i32,
    pub work_date: Option<NaiveDate>,
    pub task_id: Option<Uuid>,
    #[serde(default)]
    pub note: String,
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/time",
    tag = "operations",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Service time on the request (staff)", body = TimeSheet),
        (status = 403, description = "Not assigned staff", body = ErrorBody),
        (status = 404, description = "Unknown request", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list_time(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<TimeSheet>> {
    require_staff(&p, &state, org_id, Permission::OperateRequests).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    db::enable_staff_scope(&mut tx).await?;
    request_exists(&mut tx, request_id).await?;
    let entries = sqlx::query_as::<_, TimeEntryView>(
        "SELECT t.id, t.user_id, u.display_name AS user_name, t.task_id, t.work_date, t.minutes, t.note
         FROM time_entries t LEFT JOIN users u ON u.id = t.user_id
         WHERE t.request_id = $1 ORDER BY t.work_date DESC, t.created_at DESC",
    )
    .bind(request_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let total_minutes = entries.iter().map(|e| i64::from(e.minutes)).sum();
    Ok(Json(TimeSheet {
        entries,
        total_minutes,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/time",
    tag = "operations",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    request_body = TimeInput,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Time recorded", body = IdResponse),
        (status = 403, description = "Not assigned staff", body = ErrorBody),
        (status = 404, description = "Unknown request", body = ErrorBody),
        (status = 422, description = "Invalid minutes, date or task", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn log_time(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<TimeInput>,
) -> ApiResult<(StatusCode, Json<IdResponse>)> {
    let platform = require_staff(&p, &state, org_id, Permission::OperateRequests).await?;
    if !(1..=1440).contains(&body.minutes) {
        return Err(ApiError::Validation("minutes"));
    }
    let today = Utc::now().date_naive();
    let work_date = body.work_date.unwrap_or(today);
    if work_date > today + Duration::days(1) || work_date < today - Duration::days(366) {
        return Err(ApiError::Validation("work_date"));
    }
    let note = common::text(&body.note, 1000, true, "note")?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    db::enable_staff_scope(&mut tx).await?;
    request_exists(&mut tx, request_id).await?;
    if let Some(t) = body.task_id {
        let ok: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM tasks WHERE id = $1 AND request_id = $2)",
        )
        .bind(t)
        .bind(request_id)
        .fetch_one(&mut *tx)
        .await?;
        if !ok {
            return Err(ApiError::Validation("task_id"));
        }
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO time_entries (id, org_id, request_id, task_id, user_id, work_date, minutes, note)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(org_id)
    .bind(request_id)
    .bind(body.task_id)
    .bind(p.user_id)
    .bind(work_date)
    .bind(body.minutes)
    .bind(&note)
    .execute(&mut *tx)
    .await?;
    InternalAudit {
        p: &p,
        platform,
        customer: org_id,
    }
    .record(
        &mut tx,
        "time.logged",
        "time_entry",
        id,
        json!({ "request": request_id, "minutes": body.minutes, "work_date": work_date }),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(IdResponse { id })))
}
