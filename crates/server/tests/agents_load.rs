//! Worker load test (agent layer L9). Ignored by default; run with
//! `cargo test -p procure-server --test agents_load -- --ignored agents_load --nocapture`.
//!
//! 2,000 queued runs across 20 organisations, drained by 4 concurrent workers (each the
//! production `worker::run_next` loop against the same database). Asserts that every run is
//! executed exactly once, in its own organisation's context, that nothing one organisation's
//! runs produce is visible to another, and that all runs finish. Prints timings for
//! docs/agents/L9_REPORT.md.

// Test helpers: a failed unwrap is a failed test, which is the intent.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use common::*;
use futures_util::future::BoxFuture;
use procure_domain::agents::{Severity, Trigger};
use procure_server::agents::worker;
use procure_server::agents::{self, Agent, AgentCx, AgentError, AgentRun, NewException, Outcome};
use procure_server::db;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

const ORGS: usize = 20;
const RUNS_PER_ORG: usize = 100;
const WORKERS: usize = 4;

/// What each execution saw: (run id, the run's org, the context's org, runs of other
/// organisations visible from inside the run).
type Seen = Arc<Mutex<Vec<(Uuid, Uuid, Uuid, i64)>>>;

/// Records every execution and raises one exception per run.
struct Probe {
    seen: Seen,
}

impl Agent for Probe {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn run<'a>(
        &'a self,
        cx: &'a AgentCx,
        run: &'a AgentRun,
    ) -> BoxFuture<'a, Result<Outcome, AgentError>> {
        Box::pin(async move {
            let mut tx = cx.tx().await?;
            let foreign: i64 =
                sqlx::query_scalar("SELECT count(*) FROM agent_runs WHERE org_id <> $1")
                    .bind(cx.org_id)
                    .fetch_one(&mut *tx)
                    .await?;
            tx.commit().await?;
            self.seen
                .lock()
                .unwrap()
                .push((run.id, run.org_id, cx.org_id, foreign));
            let mut ex = NewException::new(
                "echo",
                Severity::Info,
                "runtime.test",
                &format!("load:{}", run.id),
            );
            ex.subject_type = Some("agent_run".into());
            ex.subject_id = Some(run.id);
            ex.params = json!({ "org": cx.org_id });
            Ok(Outcome {
                output: json!({ "n": run.input["n"].clone() }),
                exceptions: vec![ex],
                model_calls: 0,
            })
        })
    }
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
#[ignore = "load test: cargo test -p procure-server --test agents_load -- --ignored agents_load"]
async fn agents_load_2000_runs_20_orgs_4_workers(pool: PgPool) {
    let seen: Seen = Arc::default();
    let probe = Arc::new(Probe { seen: seen.clone() });
    let app = App::configured(pool.clone(), move |s| {
        s.agents = Arc::new(vec![probe as Arc<dyn Agent>]);
    })
    .await;
    let w = world_with(app).await;
    let app = &w.app;

    // 20 organisations (A and B with an active admin; the others created by the platform
    // admin, their admins never sign in), agents on.
    let setup = Instant::now();
    let mut orgs = vec![w.org_a, w.org_b];
    for i in orgs.len()..ORGS {
        let r = app
            .post(
                "/api/v1/platform/organisations",
                &w.super_admin,
                json!({ "name": format!("Load {i} KK"), "admin_email": format!("admin@load{i}.test"),
                        "admin_name": "Admin" }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        orgs.push(uuid(&r.body["org_id"]));
    }
    for (org, admin) in [(w.org_a, &w.admin_a), (w.org_b, &w.admin_b)] {
        let r = app
            .put(
                &format!("/api/v1/orgs/{org}/agents/settings"),
                admin,
                json!({ "enabled": true, "auto_send_supplier_mail": false,
                        "monthly_model_call_budget": 2000 }),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    }
    for org in &orgs[2..] {
        let mut tx = app.pool.begin().await.unwrap();
        db::set_org(&mut tx, *org).await.unwrap();
        sqlx::query("INSERT INTO agent_settings (org_id, enabled) VALUES ($1, true)")
            .bind(org)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    // 100 queued runs each, enqueued the way business handlers do.
    let mut owner_of: HashMap<Uuid, Uuid> = HashMap::new();
    for (o, org) in orgs.iter().enumerate() {
        let mut tx = app.pool.begin().await.unwrap();
        db::set_org(&mut tx, *org).await.unwrap();
        for n in 0..RUNS_PER_ORG {
            let id = agents::enqueue(
                &mut tx,
                *org,
                "echo",
                Trigger::Event,
                None,
                json!({ "n": o * RUNS_PER_ORG + n }),
                Some(&format!("load:{o}:{n}")),
            )
            .await
            .unwrap()
            .unwrap();
            owner_of.insert(id, *org);
        }
        tx.commit().await.unwrap();
    }
    assert_eq!(owner_of.len(), ORGS * RUNS_PER_ORG);
    let setup = setup.elapsed();

    // 4 workers drain the queue concurrently.
    let started = Instant::now();
    let mut handles = Vec::new();
    for _ in 0..WORKERS {
        let state = app.state.clone();
        handles.push(tokio::spawn(async move {
            let mut done = 0usize;
            let mut idle = 0;
            // A worker that finds nothing due looks again a few times: another worker may
            // still be holding the last runs.
            while idle < 3 {
                match worker::run_next(&state).await.unwrap() {
                    Some(_) => {
                        done += 1;
                        idle = 0;
                    }
                    None => {
                        idle += 1;
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
            done
        }));
    }
    let mut per_worker = Vec::new();
    for h in handles {
        per_worker.push(h.await.unwrap());
    }
    let drained = started.elapsed();

    // Every run executed exactly once, in its own organisation, seeing only its own data.
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), ORGS * RUNS_PER_ORG, "executions");
    let mut count: HashMap<Uuid, usize> = HashMap::new();
    for (run, run_org, cx_org, foreign) in &seen {
        *count.entry(*run).or_default() += 1;
        assert_eq!(owner_of.get(run), Some(run_org));
        assert_eq!(run_org, cx_org);
        assert_eq!(*foreign, 0, "run {run} saw other organisations' runs");
    }
    assert_eq!(count.len(), ORGS * RUNS_PER_ORG);
    assert!(count.values().all(|c| *c == 1), "a run was executed twice");
    assert_eq!(per_worker.iter().sum::<usize>(), ORGS * RUNS_PER_ORG);

    // Database state: all succeeded on the first attempt; each organisation has exactly its
    // own 100 exceptions, each raised by one of its own runs.
    // Through each organisation's own (RLS-scoped) context.
    for org in &orgs {
        let mut tx = app.pool.begin().await.unwrap();
        db::set_org(&mut tx, *org).await.unwrap();
        let runs: Vec<(Uuid, Uuid, String, i32)> =
            sqlx::query_as("SELECT id, org_id, status, attempts FROM agent_runs")
                .fetch_all(&mut *tx)
                .await
                .unwrap();
        assert_eq!(runs.len(), RUNS_PER_ORG, "{org}");
        assert!(
            runs.iter()
                .all(|r| r.1 == *org && r.2 == "succeeded" && r.3 == 1),
            "{org}"
        );
        let exs: Vec<(Uuid, Option<Uuid>, serde_json::Value)> =
            sqlx::query_as("SELECT org_id, run_id, params FROM agent_exceptions")
                .fetch_all(&mut *tx)
                .await
                .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(exs.len(), RUNS_PER_ORG, "{org}");
        for (ex_org, run, params) in exs {
            assert_eq!(ex_org, *org);
            assert_eq!(params["org"], org.to_string());
            assert_eq!(owner_of.get(&run.unwrap()), Some(org));
        }
    }
    // And through the API: an organisation's admin sees its own runs and exceptions only,
    // and is refused another organisation's.
    for (org, admin, other) in [
        (w.org_a, &w.admin_a, w.org_b),
        (w.org_b, &w.admin_b, w.org_a),
    ] {
        let r = app
            .get(&format!("/api/v1/orgs/{org}/agents/runs?limit=500"), admin)
            .await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
        assert_eq!(r.body.as_array().unwrap().len(), RUNS_PER_ORG);
        let r = app
            .get(
                &format!("/api/v1/orgs/{org}/agents/exceptions?status=open&limit=500"),
                admin,
            )
            .await;
        assert_eq!(r.body.as_array().unwrap().len(), RUNS_PER_ORG);
        for path in ["runs", "exceptions"] {
            let r = app
                .get(&format!("/api/v1/orgs/{other}/agents/{path}"), admin)
                .await;
            assert!(
                matches!(r.status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
                "{path}: {}",
                r.status
            );
        }
    }

    let runs = ORGS * RUNS_PER_ORG;
    println!(
        "agents_load: {runs} runs, {ORGS} orgs, {WORKERS} workers; setup {:.2}s; drained in {:.2}s \
         ({:.0} runs/s, {:.1} ms/run); per worker {:?}",
        setup.as_secs_f64(),
        drained.as_secs_f64(),
        runs as f64 / drained.as_secs_f64(),
        drained.as_secs_f64() * 1000.0 / runs as f64,
        per_worker
    );
}
