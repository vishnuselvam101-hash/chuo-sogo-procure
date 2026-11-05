//! Agent runtime (L1, docs/agents/DESIGN.md): queue (enqueue, claim, finish, dedupe, retry
//! with backoff, budget), exception inbox (raise with dedupe, decide with audit, the action's
//! own permission), tenant isolation and permissions. Uses a test-only `echo` agent that is
//! installed into the app state here and exists in no production registry.

// Test helpers: a failed unwrap is a failed test, which is the intent.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use common::*;
use futures_util::future::BoxFuture;
use procure_domain::agents::{RunStatus, Severity, Trigger};
use procure_domain::rbac::Permission;
use procure_server::agents::{
    self, Actor, Agent, AgentCx, AgentError, AgentException, AgentRun, NewException, Outcome,
    worker,
};
use procure_server::api::common::Audit;
use procure_server::db::{self, Tx};
use procure_server::error::ApiError;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

/// Test agent. Input: `{"fail": "retry"|"fatal", "calls": n, "raise": [{"key", "action"}]}`.
/// Actions: `note` (anyone who may decide) and `admin_only` (needs ManageAgents).
struct Echo;

impl Agent for Echo {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn run<'a>(
        &'a self,
        _cx: &'a AgentCx,
        run: &'a AgentRun,
    ) -> BoxFuture<'a, Result<Outcome, AgentError>> {
        Box::pin(async move {
            match run.input["fail"].as_str() {
                Some("retry") => return Err(AgentError::Retry("echo_retry")),
                Some("fatal") => return Err(AgentError::Fatal("echo_fatal")),
                _ => {}
            }
            let exceptions = run.input["raise"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|r| {
                            let mut ex = NewException::new(
                                "echo",
                                Severity::Warning,
                                "runtime.test",
                                &format!("echo:{}", r["key"].as_str().unwrap()),
                            );
                            ex.params = json!({ "n": r["n"].clone() });
                            ex.proposed_action = r["action"]
                                .as_str()
                                .map(|t| json!({ "type": t, "value": 1 }));
                            ex
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok(Outcome {
                output: json!({ "echo": run.input.clone() }),
                exceptions,
                model_calls: run.input["calls"].as_i64().unwrap_or(0) as i32,
            })
        })
    }

    fn apply<'a>(
        &'a self,
        tx: &'a mut Tx,
        ex: &'a AgentException,
        action: &'a Value,
        actor: &'a Actor<'a>,
    ) -> BoxFuture<'a, Result<(), ApiError>> {
        Box::pin(async move {
            match action["type"].as_str() {
                Some("note") => {}
                Some("admin_only") => actor.require(Permission::ManageAgents)?,
                _ => return Err(ApiError::Validation("unknown_action")),
            }
            Audit {
                org_id: ex.org_id,
                p: actor.principal,
            }
            .record(tx, "echo.applied", "agent_exception", ex.id, action.clone())
            .await
        })
    }
}

async fn echo_world(pool: PgPool) -> World {
    let app = App::configured(pool, |s| s.agents = Arc::new(vec![Arc::new(Echo)])).await;
    world_with(app).await
}

fn url(org: Uuid, path: &str) -> String {
    format!("/api/v1/orgs/{org}/agents/{path}")
}

async fn enable(w: &World, org: Uuid, admin: &str, budget: i32) {
    let r = w
        .app
        .put(
            &url(org, "settings"),
            admin,
            json!({ "enabled": true, "auto_send_supplier_mail": false,
                    "monthly_model_call_budget": budget }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["enabled"], true);
}

/// Queues an echo run through the manual-trigger API; returns the run id.
async fn trigger(w: &World, org: Uuid, admin: &str, subject: Uuid, input: Value) -> Uuid {
    let r = w
        .app
        .post(
            &url(org, "echo/run"),
            admin,
            json!({ "subject_type": "test", "subject_id": subject, "input": input }),
        )
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.body);
    uuid(&r.body["id"])
}

async fn run_row(w: &World, org: Uuid, token: &str, id: Uuid) -> Value {
    let r = w.app.get(&url(org, "runs?limit=500"), token).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == id.to_string())
        .cloned()
        .unwrap()
}

/// Makes a waiting run due now (as the table owner, in the run's organisation context).
async fn make_due(owner: &PgPool, org: Uuid, id: Uuid) {
    let mut tx = owner.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.org_id', $1, true)")
        .bind(org.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let n = sqlx::query("UPDATE agent_runs SET run_after = now() WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(n, 1);
    tx.commit().await.unwrap();
}

async fn audit_actions(w: &World, org: Uuid, admin: &str, action: &str) -> Vec<Value> {
    let r = w
        .app
        .get(&format!("/api/v1/orgs/{org}/audit?limit=500"), admin)
        .await;
    assert_eq!(r.body["chain_valid"], true);
    r.body["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["action"] == action)
        .cloned()
        .collect()
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn runs_are_queued_deduplicated_claimed_and_finished(pool: PgPool) {
    let w = echo_world(pool).await;
    let (org, admin) = (w.org_a, w.admin_a.as_str());

    // Off by default: nothing can be queued.
    let r = w.app.get(&url(org, "settings"), admin).await;
    assert_eq!(r.body["enabled"], false);
    assert_eq!(r.body["monthly_model_call_budget"], 2000);
    let r = w.app.post(&url(org, "echo/run"), admin, json!({})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.body["error"]["detail"], "agents_disabled");
    {
        let mut tx = w.app.pool.begin().await.unwrap();
        db::set_org(&mut tx, org).await.unwrap();
        let none = agents::enqueue(&mut tx, org, "echo", Trigger::Event, None, json!({}), None)
            .await
            .unwrap();
        assert_eq!(none, None);
        tx.commit().await.unwrap();
    }
    // Unknown agents do not exist.
    let r = w.app.post(&url(org, "nope/run"), admin, json!({})).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    enable(&w, org, admin, 2000).await;
    let subject = Uuid::now_v7();
    let id = trigger(&w, org, admin, subject, json!({ "x": 1 })).await;
    // The same request while it waits is the same run.
    assert_eq!(
        trigger(&w, org, admin, subject, json!({ "x": 1 })).await,
        id
    );

    // An event hook enqueues inside its own transaction, with its own key.
    let event_id = {
        let mut tx = w.app.pool.begin().await.unwrap();
        db::set_org(&mut tx, org).await.unwrap();
        let a = agents::enqueue(
            &mut tx,
            org,
            "echo",
            Trigger::Event,
            Some(("test", subject)),
            json!({}),
            Some("echo:event:1"),
        )
        .await
        .unwrap()
        .unwrap();
        let b = agents::enqueue(
            &mut tx,
            org,
            "echo",
            Trigger::Event,
            None,
            json!({}),
            Some("echo:event:1"),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(a, b);
        tx.commit().await.unwrap();
        a
    };
    assert_ne!(event_id, id);

    let mut done = vec![];
    while let Some((run, status)) = worker::run_next(&w.app.state).await.unwrap() {
        assert_eq!(status, RunStatus::Succeeded);
        done.push(run);
    }
    done.sort();
    let mut expected = vec![id, event_id];
    expected.sort();
    assert_eq!(done, expected);

    let row = run_row(&w, org, admin, id).await;
    assert_eq!(row["status"], "succeeded");
    assert_eq!(row["trigger"], "manual");
    assert_eq!(row["attempts"], 1);
    assert_eq!(row["output"]["echo"]["x"], 1);
    assert_eq!(row["created_by"], w.user_id(admin).await.to_string());
    assert!(row["finished_at"].is_string());

    // Finished runs no longer block the key.
    let again = trigger(&w, org, admin, subject, json!({})).await;
    assert_ne!(again, id);

    let r = w.app.get(&url(org, "runs?status=succeeded"), admin).await;
    assert_eq!(r.body.as_array().unwrap().len(), 2);
    let r = w.app.get(&url(org, "runs?status=bogus"), admin).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = w.app.get(&url(org, "summary"), admin).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["enabled"], true);
    let by: Vec<(String, i64)> = r.body["runs_by_status"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["key"].as_str().unwrap().into(),
                c["count"].as_i64().unwrap(),
            )
        })
        .collect();
    assert!(by.contains(&("succeeded".into(), 2)) && by.contains(&("queued".into(), 1)));
    assert_eq!(
        audit_actions(&w, org, admin, "agent.settings_updated")
            .await
            .len(),
        1
    );
    assert_eq!(
        audit_actions(&w, org, admin, "agent.run_requested")
            .await
            .len(),
        3
    );
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn failures_are_retried_with_backoff_then_failed(pool: PgPool) {
    let owner = pool.clone();
    let w = echo_world(pool).await;
    let (org, admin) = (w.org_a, w.admin_a.as_str());
    enable(&w, org, admin, 2000).await;

    let id = trigger(&w, org, admin, Uuid::now_v7(), json!({ "fail": "retry" })).await;
    for attempt in 1..=3 {
        let (run, status) = worker::run_next(&w.app.state).await.unwrap().unwrap();
        assert_eq!(run, id);
        let row = run_row(&w, org, admin, id).await;
        assert_eq!(row["attempts"], attempt);
        assert_eq!(row["error_code"], "echo_retry");
        if attempt < 3 {
            assert_eq!(status, RunStatus::Queued);
            assert_eq!(row["status"], "queued");
            // Backing off: not due again yet.
            let after: chrono::DateTime<chrono::Utc> =
                serde_json::from_value(row["run_after"].clone()).unwrap();
            assert!(after > chrono::Utc::now() + chrono::Duration::seconds(50));
            assert!(worker::run_next(&w.app.state).await.unwrap().is_none());
            make_due(&owner, org, id).await;
        } else {
            assert_eq!(status, RunStatus::Failed);
            assert_eq!(row["status"], "failed");
        }
    }
    // A fatal error is not retried.
    let fatal = trigger(&w, org, admin, Uuid::now_v7(), json!({ "fail": "fatal" })).await;
    let (run, status) = worker::run_next(&w.app.state).await.unwrap().unwrap();
    assert_eq!((run, status), (fatal, RunStatus::Failed));
    assert_eq!(run_row(&w, org, admin, fatal).await["attempts"], 1);
    let failed = audit_actions(&w, org, admin, "agent.run_failed").await;
    assert_eq!(failed.len(), 2);
    assert!(failed.iter().all(|e| e["actor_user_id"].is_null()));
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn runs_are_skipped_once_the_monthly_budget_is_used(pool: PgPool) {
    let w = echo_world(pool).await;
    let (org, admin) = (w.org_a, w.admin_a.as_str());
    enable(&w, org, admin, 2).await;
    let first = trigger(&w, org, admin, Uuid::now_v7(), json!({ "calls": 2 })).await;
    assert_eq!(
        worker::run_next(&w.app.state).await.unwrap(),
        Some((first, RunStatus::Succeeded))
    );
    assert_eq!(run_row(&w, org, admin, first).await["model_calls"], 2);
    let second = trigger(&w, org, admin, Uuid::now_v7(), json!({})).await;
    assert_eq!(
        worker::run_next(&w.app.state).await.unwrap(),
        Some((second, RunStatus::Skipped))
    );
    let row = run_row(&w, org, admin, second).await;
    assert_eq!(row["status"], "skipped");
    assert_eq!(row["error_code"], "budget_exhausted");
    let s = w.app.get(&url(org, "summary"), admin).await;
    assert_eq!(s.body["model_calls_this_month"], 2);
    assert_eq!(s.body["monthly_model_call_budget"], 2);

    // Budgets are validated.
    let r = w
        .app
        .put(
            &url(org, "settings"),
            admin,
            json!({ "enabled": true, "auto_send_supplier_mail": false,
                    "monthly_model_call_budget": -1 }),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
}

/// Runs echo once with these exceptions to raise; returns the open exceptions.
async fn raise(w: &World, org: Uuid, admin: &str, raise: Value) -> Vec<Value> {
    trigger(w, org, admin, Uuid::now_v7(), json!({ "raise": raise })).await;
    let (_, status) = worker::run_next(&w.app.state).await.unwrap().unwrap();
    assert_eq!(status, RunStatus::Succeeded);
    let r = w.app.get(&url(org, "exceptions"), admin).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body.as_array().unwrap().clone()
}

fn by_key<'a>(list: &'a [Value], key: &str) -> &'a Value {
    list.iter()
        .find(|e| e["dedupe_key"] == format!("echo:{key}"))
        .unwrap()
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn exceptions_are_deduplicated_and_decided_with_audit(pool: PgPool) {
    let w = echo_world(pool).await;
    let (org, admin) = (w.org_a, w.admin_a.as_str());
    enable(&w, org, admin, 2000).await;
    let manager = w
        .member(org, admin, "buyer@a.test", "procurement_manager")
        .await;

    let list = raise(
        &w,
        org,
        admin,
        json!([{ "key": "a", "n": 1, "action": "note" },
               { "key": "b", "n": 1, "action": "note" },
               { "key": "c", "n": 1 }]),
    )
    .await;
    assert_eq!(list.len(), 3);
    // The same finding again: still one open exception per key, nothing new audited.
    let list = raise(
        &w,
        org,
        admin,
        json!([{ "key": "a", "n": 1, "action": "note" }]),
    )
    .await;
    assert_eq!(list.len(), 3);
    assert_eq!(
        audit_actions(&w, org, admin, "agent.exception_raised")
            .await
            .len(),
        3
    );
    assert!(
        audit_actions(&w, org, admin, "agent.exception_updated")
            .await
            .is_empty()
    );
    // A changed finding refreshes the open one.
    let list = raise(
        &w,
        org,
        admin,
        json!([{ "key": "a", "n": 2, "action": "note" }]),
    )
    .await;
    assert_eq!(list.len(), 3);
    let a = by_key(&list, "a").clone();
    assert_eq!(a["params"]["n"], 2);
    assert_eq!(a["title_key"], "runtime.test");
    assert_eq!(a["severity"], "warning");
    assert_eq!(
        audit_actions(&w, org, admin, "agent.exception_updated")
            .await
            .len(),
        1
    );

    let decide = |id: &Value, body: Value| {
        let path = url(org, &format!("exceptions/{}/decide", id.as_str().unwrap()));
        let app = &w.app;
        let token = manager.clone();
        async move { app.post(&path, &token, body).await }
    };
    // Approve: decided, audited, and the action executed by the agent.
    let r = decide(&a["id"], json!({ "decision": "approve", "note": "ok" })).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["status"], "approved");
    assert_eq!(r.body["decided_action"]["type"], "note");
    assert_eq!(r.body["decision_note"], "ok");
    let decided = audit_actions(&w, org, admin, "agent.exception_decided").await;
    assert_eq!(decided.len(), 1);
    assert_eq!(decided[0]["data"]["decision"], "approve");
    assert_eq!(audit_actions(&w, org, admin, "echo.applied").await.len(), 1);
    // Only once.
    let r = decide(&a["id"], json!({ "decision": "reject" })).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.body["error"]["detail"], "exception_not_open");

    // A new finding with a decided key opens a new exception.
    let list = raise(
        &w,
        org,
        admin,
        json!([{ "key": "a", "n": 3, "action": "note" }]),
    )
    .await;
    assert_ne!(by_key(&list, "a")["id"], a["id"]);

    // Edit: same action type only; the edited action is what runs.
    let b = by_key(&list, "b").clone();
    let r = decide(
        &b["id"],
        json!({ "decision": "edit", "action": { "type": "admin_only" } }),
    )
    .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    let r = decide(
        &b["id"],
        json!({ "decision": "edit", "action": { "type": "note", "value": 7 } }),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["status"], "edited");
    assert_eq!(r.body["decided_action"]["value"], 7);
    let applied = audit_actions(&w, org, admin, "echo.applied").await;
    assert_eq!(applied.len(), 2);
    assert_eq!(applied[0]["data"]["value"], 7);

    // Reject: nothing executed. A notice without action can be approved (acknowledged).
    let c = by_key(&list, "c").clone();
    let r = decide(&c["id"], json!({ "decision": "reject" })).await;
    assert_eq!(r.body["status"], "rejected");
    assert_eq!(audit_actions(&w, org, admin, "echo.applied").await.len(), 2);
    let r = decide(&c["id"], json!({ "decision": "approve" })).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let r = decide(&c["id"], json!({ "decision": "maybe" })).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);

    let r = w.app.get(&url(org, "exceptions?status=all"), admin).await;
    assert_eq!(r.body.as_array().unwrap().len(), 4);
    let r = w
        .app
        .get(&url(org, "exceptions?status=rejected"), admin)
        .await;
    assert_eq!(r.body.as_array().unwrap().len(), 1);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn actions_need_their_own_permission_and_a_refusal_undoes_the_decision(pool: PgPool) {
    let w = echo_world(pool).await;
    let (org, admin) = (w.org_a, w.admin_a.as_str());
    enable(&w, org, admin, 2000).await;
    let manager = w
        .member(org, admin, "buyer@a.test", "procurement_manager")
        .await;
    let list = raise(
        &w,
        org,
        admin,
        json!([{ "key": "x", "action": "admin_only" }]),
    )
    .await;
    let path = url(
        org,
        &format!("exceptions/{}/decide", list[0]["id"].as_str().unwrap()),
    );
    // A manager may decide, but not perform this action: nothing changes.
    let r = w
        .app
        .post(&path, &manager, json!({ "decision": "approve" }))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let open = w.app.get(&url(org, "exceptions"), admin).await;
    assert_eq!(open.body[0]["status"], "open");
    assert!(
        audit_actions(&w, org, admin, "agent.exception_decided")
            .await
            .is_empty()
    );
    // The organisation's admin may.
    let r = w
        .app
        .post(&path, admin, json!({ "decision": "approve" }))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(audit_actions(&w, org, admin, "echo.applied").await.len(), 1);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn other_tenants_and_roles_without_permission_see_nothing(pool: PgPool) {
    let w = echo_world(pool).await;
    let (org, admin) = (w.org_a, w.admin_a.as_str());
    enable(&w, org, admin, 2000).await;
    let list = raise(&w, org, admin, json!([{ "key": "t", "action": "note" }])).await;
    let ex = list[0]["id"].as_str().unwrap().to_string();
    let other = w.admin_b.as_str();

    // Org B: org A does not exist for it.
    for path in ["settings", "runs", "exceptions", "summary"] {
        let r = w.app.get(&url(org, path), other).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}");
    }
    let r = w
        .app
        .post(
            &url(org, &format!("exceptions/{ex}/decide")),
            other,
            json!({ "decision": "approve" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // Through its own org path, A's exception is not there either (RLS + org filter).
    let r = w
        .app
        .post(
            &url(w.org_b, &format!("exceptions/{ex}/decide")),
            other,
            json!({ "decision": "approve" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = w
        .app
        .get(&url(w.org_b, "exceptions?status=all"), other)
        .await;
    assert_eq!(r.body.as_array().unwrap().len(), 0);
    let r = w.app.get(&url(w.org_b, "runs"), other).await;
    assert_eq!(r.body.as_array().unwrap().len(), 0);
    // The database itself: in B's context A's rows do not exist.
    let mut tx = w.app.pool.begin().await.unwrap();
    db::set_org(&mut tx, w.org_b).await.unwrap();
    let (runs, exs): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM agent_runs), (SELECT count(*) FROM agent_exceptions)",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!((runs, exs), (0, 0));
    tx.rollback().await.unwrap();

    // Roles: a requester sees nothing; a manager decides but does not manage.
    let requester = w.member(org, admin, "req@a.test", "requester").await;
    for path in ["settings", "runs", "exceptions", "summary"] {
        let r = w.app.get(&url(org, path), &requester).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{path}");
    }
    let r = w
        .app
        .post(
            &url(org, &format!("exceptions/{ex}/decide")),
            &requester,
            json!({ "decision": "approve" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let manager = w
        .member(org, admin, "buyer@a.test", "procurement_manager")
        .await;
    assert_eq!(
        w.app.get(&url(org, "runs"), &manager).await.status,
        StatusCode::OK
    );
    let r = w
        .app
        .put(
            &url(org, "settings"),
            &manager,
            json!({ "enabled": false, "auto_send_supplier_mail": true,
                    "monthly_model_call_budget": 5 }),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = w.app.post(&url(org, "echo/run"), &manager, json!({})).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn assigned_staff_see_and_decide_exceptions_of_their_customers(pool: PgPool) {
    let w = echo_world(pool).await;
    let (org, admin) = (w.org_a, w.admin_a.as_str());
    enable(&w, org, admin, 2000).await;
    enable(&w, w.org_b, &w.admin_b, 2000).await;
    raise(&w, org, admin, json!([{ "key": "s", "action": "note" }])).await;
    raise(
        &w,
        w.org_b,
        &w.admin_b,
        json!([{ "key": "s", "action": "note" }]),
    )
    .await;

    let operator = w
        .member(
            w.platform_org,
            &w.super_admin,
            "op@chuo-sogo.test",
            "procurement_operator",
        )
        .await;
    // Not assigned: nothing.
    let r = w.app.get(&url(org, "exceptions"), &operator).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let op_id = w.user_id(&operator).await;
    let r = w
        .app
        .post(
            &format!("/api/v1/orgs/{org}/assignments"),
            &w.super_admin,
            json!({ "user_id": op_id }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);

    // Staff console: only the assigned customer's exceptions.
    let session = admin_session(&w.app, "op@chuo-sogo.test").await;
    let r = w
        .app
        .get("/api/admin/v1/ops/agents/exceptions", &session)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let list = r.body.as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["exception"]["org_id"], org.to_string());
    assert_eq!(list[0]["org_name"], "株式会社エー製作所");

    // They may decide for the assigned customer, but never change its agent settings.
    let id = list[0]["exception"]["id"].as_str().unwrap();
    let r = w
        .app
        .post(
            &url(org, &format!("exceptions/{id}/decide")),
            &operator,
            json!({ "decision": "approve" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let r = w
        .app
        .put(
            &url(org, "settings"),
            &operator,
            json!({ "enabled": false, "auto_send_supplier_mail": false,
                    "monthly_model_call_budget": 1 }),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

// Admin console session (second factor enrolled on first sign-in).
async fn admin_session(app: &App, email: &str) -> String {
    use procure_server::mfa;
    let post = |path: &str, body: Value| {
        let uri = format!("/api/admin/v1{path}");
        async move {
            app.call(axum::http::Method::POST, &uri, None, Some(body))
                .await
        }
    };
    let r = post(
        "/auth/login",
        json!({ "email": email, "password": PASSWORD }),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let token = r.body["token"].as_str().unwrap().to_string();
    let e = post("/auth/totp/enroll", json!({ "token": token })).await;
    let secret = mfa::base32_decode(e.body["secret"].as_str().unwrap()).unwrap();
    let step = chrono::Utc::now().timestamp().div_euclid(mfa::STEP_SECONDS);
    let code = format!("{:06}", mfa::code_at(&secret, step));
    let c = post(
        "/auth/totp/confirm",
        json!({ "token": token, "code": code }),
    )
    .await;
    assert_eq!(c.status, StatusCode::OK, "{:?}", c.body);
    c.body["token"].as_str().unwrap().to_string()
}
