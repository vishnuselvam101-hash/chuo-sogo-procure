//! The background worker: claims queued runs across organisations, runs them, records the
//! outcome (exceptions raised in the same transaction that ends the run), retries failures
//! with backoff, and queues periodic sweeps for organisations with agents on.

use std::time::Duration;

use procure_domain::agents::{self as rules, RunStatus, Trigger};
use serde_json::json;
use uuid::Uuid;

use super::queue::{self, AgentRun};
use super::{AgentCx, AgentError, exceptions, find};
use crate::AppState;
use crate::audit::{self, Event};
use crate::db;

/// A run that takes longer is abandoned and retried.
pub const RUN_TIMEOUT: Duration = Duration::from_secs(600);
const TICK: Duration = Duration::from_secs(15);
/// Sweeps are checked every this many ticks.
const SWEEP_EVERY: u32 = 4;
/// Runs one tick works through at most.
const BATCH: usize = 20;

/// Starts the worker loop (main.rs).
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(TICK);
        let mut n: u32 = 0;
        loop {
            tick.tick().await;
            if n.is_multiple_of(SWEEP_EVERY)
                && let Err(e) = enqueue_scheduled(&state).await
            {
                tracing::warn!(error = %e, "agent sweeps");
            }
            n = n.wrapping_add(1);
            match run_due(&state, BATCH).await {
                Ok(0) => {}
                Ok(done) => tracing::info!(runs = done, "agent runs processed"),
                Err(e) => tracing::warn!(error = %e, "agent worker"),
            }
        }
    });
}

/// Claims and runs due runs until none is left or `max` were processed.
pub async fn run_due(state: &AppState, max: usize) -> Result<usize, sqlx::Error> {
    let mut done = 0;
    while done < max {
        if run_next(state).await?.is_none() {
            break;
        }
        done += 1;
    }
    Ok(done)
}

/// Claims one due run and runs it. Returns the run id and the status it ended in (`queued`
/// when it will be retried); `None` when nothing is due.
pub async fn run_next(state: &AppState) -> Result<Option<(Uuid, RunStatus)>, sqlx::Error> {
    let Some(run) = queue::claim(&state.pool).await? else {
        return Ok(None);
    };
    let status = execute(state, &run).await?;
    Ok(Some((run.id, status)))
}

async fn execute(state: &AppState, run: &AgentRun) -> Result<RunStatus, sqlx::Error> {
    let org = run.org_id;
    let Some(agent) = find(state, &run.agent) else {
        return fail(state, run, "unknown_agent", false, 0).await;
    };
    if run.attempts > rules::MAX_ATTEMPTS {
        // Its worker died (lease ran out) on the last attempt.
        return fail(state, run, "lease_expired", false, 0).await;
    }
    {
        let mut tx = state.pool.begin().await?;
        db::set_org(&mut tx, org).await?;
        let used = queue::model_calls_this_month(&mut tx, org).await?;
        let budget = queue::budget(&mut tx, org).await?;
        if !rules::within_budget(used, budget) {
            queue::skip(&mut tx, run.id, "budget_exhausted").await?;
            audit_run(&mut tx, run, "agent.run_skipped", "budget_exhausted").await?;
            tx.commit().await?;
            return Ok(RunStatus::Skipped);
        }
        tx.commit().await?;
    }
    let cx = AgentCx::new(state, org, run.id);
    let result = match tokio::time::timeout(RUN_TIMEOUT, agent.run(&cx, run)).await {
        Ok(r) => r,
        Err(_) => Err(AgentError::Retry("timeout")),
    };
    match result {
        Ok(outcome) => {
            let calls = outcome.model_calls.max(cx.model_calls());
            let saved: Result<(), sqlx::Error> = async {
                let mut tx = state.pool.begin().await?;
                db::set_org(&mut tx, org).await?;
                for ex in &outcome.exceptions {
                    exceptions::raise(&mut tx, org, agent.name(), Some(run.id), ex).await?;
                }
                queue::finish(&mut tx, run.id, &outcome.output, calls).await?;
                tx.commit().await
            }
            .await;
            match saved {
                Ok(()) => Ok(RunStatus::Succeeded),
                // The outcome breaks a constraint (e.g. a malformed title key): a bug in the
                // agent, not worth retrying.
                Err(sqlx::Error::Database(e))
                    if e.is_check_violation() || e.is_foreign_key_violation() =>
                {
                    tracing::warn!(run = %run.id, agent = %run.agent, error = %e, "invalid agent outcome");
                    fail(state, run, "invalid_outcome", false, calls).await
                }
                Err(e) => {
                    tracing::warn!(run = %run.id, error = %e, "agent outcome not saved");
                    fail(state, run, "database_error", true, calls).await
                }
            }
        }
        Err(e) => {
            tracing::warn!(run = %run.id, agent = %run.agent, error = %e, "agent run failed");
            fail(state, run, e.code(), e.retryable(), cx.model_calls()).await
        }
    }
}

async fn fail(
    state: &AppState,
    run: &AgentRun,
    code: &str,
    retry: bool,
    model_calls: i32,
) -> Result<RunStatus, sqlx::Error> {
    let mut tx = state.pool.begin().await?;
    db::set_org(&mut tx, run.org_id).await?;
    let status = queue::fail(&mut tx, run, code, retry, model_calls).await?;
    if status == RunStatus::Failed {
        audit_run(&mut tx, run, "agent.run_failed", code).await?;
    }
    tx.commit().await?;
    Ok(status)
}

async fn audit_run(
    tx: &mut db::Tx,
    run: &AgentRun,
    action: &str,
    code: &str,
) -> Result<(), sqlx::Error> {
    audit::record(
        tx,
        Event {
            org_id: run.org_id,
            actor: None,
            action,
            entity_type: "agent_run",
            entity_id: Some(run.id.to_string()),
            data: json!({ "agent": run.agent, "error_code": code, "attempts": run.attempts }),
            request_id: None,
        },
    )
    .await
}

/// Queues the periodic sweep of every scheduled agent for every organisation with agents
/// on, unless one was queued within its interval. Returns how many were queued.
pub async fn enqueue_scheduled(state: &AppState) -> Result<usize, sqlx::Error> {
    let scheduled: Vec<(&'static str, Duration)> = state
        .agents
        .iter()
        .filter_map(|a| a.schedule().map(|every| (a.name(), every)))
        .collect();
    if scheduled.is_empty() {
        return Ok(0);
    }
    let mut queued = 0;
    for org in queue::enabled_orgs(&state.pool).await? {
        let mut tx = state.pool.begin().await?;
        db::set_org(&mut tx, org).await?;
        for (name, every) in &scheduled {
            let recent: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM agent_runs
                                WHERE org_id = $1 AND agent = $2 AND trigger = 'schedule'
                                  AND created_at > now() - make_interval(secs => $3))",
            )
            .bind(org)
            .bind(name)
            .bind(every.as_secs_f64())
            .fetch_one(&mut *tx)
            .await?;
            if !recent
                && queue::enqueue(
                    &mut tx,
                    org,
                    name,
                    Trigger::Schedule,
                    None,
                    json!({}),
                    Some(&format!("schedule:{name}")),
                )
                .await?
                .is_some()
            {
                queued += 1;
            }
        }
        tx.commit().await?;
    }
    Ok(queued)
}
