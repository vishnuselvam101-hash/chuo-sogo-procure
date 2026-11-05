//! `agent_runs`: the job queue. Business handlers enqueue inside their own transaction;
//! the worker claims runs with `FOR UPDATE SKIP LOCKED`, one organisation context at a time.

use chrono::{DateTime, Utc};
use procure_domain::agents::{self as rules, RunStatus, Trigger};
use rand::seq::SliceRandom;
use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::{self, Tx};

/// How long a claimed run is left alone by other workers.
pub const LEASE_SECONDS: f64 = 900.0;

#[derive(Debug, Clone, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct AgentRun {
    pub id: Uuid,
    pub org_id: Uuid,
    pub agent: String,
    /// `schedule` | `event` | `manual`
    pub trigger: String,
    pub subject_type: Option<String>,
    pub subject_id: Option<Uuid>,
    /// `queued` | `running` | `succeeded` | `failed` | `skipped`
    pub status: String,
    pub attempts: i32,
    pub run_after: DateTime<Utc>,
    pub locked_until: Option<DateTime<Utc>>,
    pub dedupe_key: Option<String>,
    #[schema(value_type = Object)]
    pub input: Value,
    #[schema(value_type = Option<Object>)]
    pub output: Option<Value>,
    pub error_code: Option<String>,
    pub model_calls: i32,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

pub(crate) const RUN_COLUMNS: &str = "id, org_id, agent, trigger, subject_type, subject_id, \
     status, attempts, run_after, locked_until, dedupe_key, input, output, error_code, \
     model_calls, created_by, created_at, started_at, finished_at";

/// Whether agents are switched on for the organisation of `tx`'s context.
pub async fn enabled(tx: &mut Tx, org_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT coalesce((SELECT enabled FROM agent_settings WHERE org_id = $1), false)",
    )
    .bind(org_id)
    .fetch_one(&mut **tx)
    .await
}

/// Queues a run inside the caller's transaction (already in `org_id`'s context; e.g.
/// `record_invoice` enqueues `matching`). Returns the run id, or the id of the queued or
/// running run with the same `dedupe_key`; `None` when agents are off for the organisation
/// (nothing is queued).
pub async fn enqueue(
    tx: &mut Tx,
    org_id: Uuid,
    agent: &str,
    trigger: Trigger,
    subject: Option<(&str, Uuid)>,
    input: Value,
    dedupe_key: Option<&str>,
) -> Result<Option<Uuid>, sqlx::Error> {
    enqueue_by(tx, org_id, agent, trigger, subject, input, dedupe_key, None).await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn enqueue_by(
    tx: &mut Tx,
    org_id: Uuid,
    agent: &str,
    trigger: Trigger,
    subject: Option<(&str, Uuid)>,
    input: Value,
    dedupe_key: Option<&str>,
    created_by: Option<Uuid>,
) -> Result<Option<Uuid>, sqlx::Error> {
    if !enabled(tx, org_id).await? {
        return Ok(None);
    }
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO agent_runs (id, org_id, agent, trigger, subject_type, subject_id, input,
                                 dedupe_key, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         ON CONFLICT (org_id, dedupe_key)
             WHERE dedupe_key IS NOT NULL AND status IN ('queued', 'running')
         DO NOTHING
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(org_id)
    .bind(agent)
    .bind(trigger.code())
    .bind(subject.map(|s| s.0))
    .bind(subject.map(|s| s.1))
    .bind(input)
    .bind(dedupe_key)
    .bind(created_by)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(id) = inserted {
        return Ok(Some(id));
    }
    sqlx::query_scalar(
        "SELECT id FROM agent_runs
         WHERE org_id = $1 AND dedupe_key = $2 AND status IN ('queued', 'running')",
    )
    .bind(org_id)
    .bind(dedupe_key)
    .fetch_optional(&mut **tx)
    .await
}

/// Organisations with agents switched on (platform scope, read only).
pub async fn enabled_orgs(pool: &PgPool) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    db::enable_platform_scope(&mut tx).await?;
    let ids = sqlx::query_scalar(
        "SELECT s.org_id FROM agent_settings s JOIN organisations o ON o.id = s.org_id
         WHERE s.enabled AND o.status = 'active'",
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(ids)
}

/// Claims the next due run of any organisation with agents on (or a run whose worker's
/// lease ran out): marks it running, counts the attempt, and commits before it is run.
pub async fn claim(pool: &PgPool) -> Result<Option<AgentRun>, sqlx::Error> {
    let mut orgs = enabled_orgs(pool).await?;
    // No organisation waits behind another one's backlog.
    orgs.shuffle(&mut rand::rng());
    for org in orgs {
        let mut tx = pool.begin().await?;
        db::set_org(&mut tx, org).await?;
        let run = sqlx::query_as::<_, AgentRun>(&format!(
            "UPDATE agent_runs r SET status = 'running', attempts = r.attempts + 1,
                    started_at = now(), locked_until = now() + make_interval(secs => $1)
             WHERE r.id = (SELECT id FROM agent_runs
                           WHERE (status = 'queued' AND run_after <= now())
                              OR (status = 'running' AND locked_until < now())
                           ORDER BY run_after LIMIT 1 FOR UPDATE SKIP LOCKED)
             RETURNING {RUN_COLUMNS}"
        ))
        .bind(LEASE_SECONDS)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        if run.is_some() {
            return Ok(run);
        }
    }
    Ok(None)
}

/// Marks a running run succeeded (transaction in the run's organisation).
pub async fn finish(
    tx: &mut Tx,
    run_id: Uuid,
    output: &Value,
    model_calls: i32,
) -> Result<(), sqlx::Error> {
    end(
        tx,
        run_id,
        RunStatus::Succeeded,
        Some(output),
        None,
        model_calls,
    )
    .await
}

/// Ends a running run without doing its work (e.g. `budget_exhausted`).
pub async fn skip(tx: &mut Tx, run_id: Uuid, code: &str) -> Result<(), sqlx::Error> {
    end(tx, run_id, RunStatus::Skipped, None, Some(code), 0).await
}

async fn end(
    tx: &mut Tx,
    run_id: Uuid,
    status: RunStatus,
    output: Option<&Value>,
    code: Option<&str>,
    model_calls: i32,
) -> Result<(), sqlx::Error> {
    debug_assert!(RunStatus::Running.can_move_to(status));
    sqlx::query(
        "UPDATE agent_runs SET status = $2, output = $3, error_code = $4,
                model_calls = model_calls + $5, finished_at = now(), locked_until = NULL
         WHERE id = $1 AND status = 'running'",
    )
    .bind(run_id)
    .bind(status.code())
    .bind(output)
    .bind(code)
    .bind(model_calls.max(0))
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

/// Records a failed attempt: back in the queue after the backoff while attempts remain and
/// the error is worth retrying, otherwise `failed`. Returns the status the run ends in.
pub async fn fail(
    tx: &mut Tx,
    run: &AgentRun,
    code: &str,
    retry: bool,
    model_calls: i32,
) -> Result<RunStatus, sqlx::Error> {
    if retry && rules::retries(run.attempts) {
        sqlx::query(
            "UPDATE agent_runs SET status = 'queued', error_code = $2,
                    model_calls = model_calls + $3, locked_until = NULL,
                    run_after = now() + make_interval(secs => $4)
             WHERE id = $1 AND status = 'running'",
        )
        .bind(run.id)
        .bind(code)
        .bind(model_calls.max(0))
        .bind(rules::backoff_seconds(run.attempts) as f64)
        .execute(&mut **tx)
        .await?;
        Ok(RunStatus::Queued)
    } else {
        end(tx, run.id, RunStatus::Failed, None, Some(code), model_calls).await?;
        Ok(RunStatus::Failed)
    }
}

/// Model calls of the organisation's runs that started this calendar month, in the
/// organisation's own time zone (a run queued last month but run now counts now).
pub async fn model_calls_this_month(tx: &mut Tx, org_id: Uuid) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "WITH z AS (SELECT coalesce((SELECT timezone FROM organisations WHERE id = $1),
                                    'Asia/Tokyo') AS tz)
         SELECT coalesce(sum(r.model_calls), 0)::bigint FROM agent_runs r, z
         WHERE r.org_id = $1
           AND r.started_at >= date_trunc('month', now() AT TIME ZONE z.tz) AT TIME ZONE z.tz",
    )
    .bind(org_id)
    .fetch_one(&mut **tx)
    .await
}

/// The organisation's monthly model-call budget (the default when never set).
pub async fn budget(tx: &mut Tx, org_id: Uuid) -> Result<i32, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT coalesce((SELECT monthly_model_call_budget FROM agent_settings WHERE org_id = $1),
                         2000)",
    )
    .bind(org_id)
    .fetch_one(&mut **tx)
    .await
}
