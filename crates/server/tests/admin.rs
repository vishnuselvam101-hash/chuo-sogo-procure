//! Phase A acceptance: admin console API — second factor, separated sessions, network
//! restriction, organisation and account administration, rules, monitoring.

// Test helpers: a failed unwrap is a failed test, which is the intent.
#![allow(clippy::unwrap_used)]

mod common;

use axum::http::StatusCode;
use common::*;
use procure_server::mfa;
use serde_json::{Value, json};
use sqlx::PgPool;

const ADMIN: &str = "/api/admin/v1";

fn code(secret_b32: &str, offset_steps: i64) -> String {
    let secret = mfa::base32_decode(secret_b32).unwrap();
    let step = chrono::Utc::now().timestamp().div_euclid(mfa::STEP_SECONDS) + offset_steps;
    format!("{:06}", mfa::code_at(&secret, step))
}

async fn post(app: &App, path: &str, token: Option<&str>, body: Value) -> Resp {
    app.call(
        axum::http::Method::POST,
        &format!("{ADMIN}{path}"),
        token,
        Some(body),
    )
    .await
}

/// Full first sign-in (password → enrol → confirm). Returns (admin session, base32 secret).
async fn first_admin_login(app: &App, email: &str) -> (String, String) {
    let r = post(
        app,
        "/auth/login",
        None,
        json!({ "email": email, "password": PASSWORD }),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["step"], "enroll");
    let token = r.body["token"].as_str().unwrap().to_string();
    let e = post(app, "/auth/totp/enroll", None, json!({ "token": token })).await;
    assert_eq!(e.status, StatusCode::OK, "{:?}", e.body);
    let secret = e.body["secret"].as_str().unwrap().to_string();
    assert!(
        e.body["otpauth_uri"]
            .as_str()
            .unwrap()
            .starts_with("otpauth://totp/")
    );
    let c = post(
        app,
        "/auth/totp/confirm",
        None,
        json!({ "token": token, "code": code(&secret, 0) }),
    )
    .await;
    assert_eq!(c.status, StatusCode::OK, "{:?}", c.body);
    (c.body["token"].as_str().unwrap().to_string(), secret)
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn admin_sign_in_needs_a_second_factor_and_sessions_do_not_cross(pool: PgPool) {
    let w = world(pool).await;
    let app = &w.app;
    let (admin, secret) = first_admin_login(app, "root@chuo-sogo.test").await;
    let me = app.get(&format!("{ADMIN}/me"), &admin).await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.body["role"], "platform_super_admin");
    // An admin session does not work in the customer API, nor an app session in the admin API.
    assert_eq!(
        app.get("/api/v1/me", &admin).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.get(&format!("{ADMIN}/me"), &w.super_admin).await.status,
        StatusCode::UNAUTHORIZED
    );

    // Second sign-in: password, then the authenticator code. Wrong and replayed codes fail.
    let r = post(
        app,
        "/auth/login",
        None,
        json!({ "email": "root@chuo-sogo.test", "password": PASSWORD }),
    )
    .await;
    assert_eq!(r.body["step"], "mfa");
    let challenge = r.body["token"].as_str().unwrap().to_string();
    let wrong = post(
        app,
        "/auth/mfa",
        None,
        json!({ "token": challenge, "code": "000000" }),
    )
    .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    // The code used at enrolment cannot be replayed.
    let replay = post(
        app,
        "/auth/mfa",
        None,
        json!({ "token": challenge, "code": code(&secret, 0) }),
    )
    .await;
    assert_eq!(replay.status, StatusCode::UNAUTHORIZED);
    let ok = post(
        app,
        "/auth/mfa",
        None,
        json!({ "token": challenge, "code": code(&secret, 1) }),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK, "{:?}", ok.body);
    // The challenge is single-use.
    let again = post(
        app,
        "/auth/mfa",
        None,
        json!({ "token": challenge, "code": code(&secret, 1) }),
    )
    .await;
    assert_eq!(again.status, StatusCode::UNAUTHORIZED);

    // Customer accounts cannot use the admin console (same answer as a wrong password).
    let r = post(
        app,
        "/auth/login",
        None,
        json!({ "email": "admin@a.test", "password": PASSWORD }),
    )
    .await;
    assert_eq!(r.code(), "invalid_credentials");

    // Logout ends the admin session.
    let new = ok.body["token"].as_str().unwrap();
    assert_eq!(
        post(app, "/auth/logout", Some(new), json!({})).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.get(&format!("{ADMIN}/me"), new).await.status,
        StatusCode::UNAUTHORIZED
    );
    // Sign-ins are audited in the platform log.
    let audit = app
        .get(&format!("{ADMIN}/orgs/{}/audit", w.platform_org), &admin)
        .await;
    let actions: Vec<_> = audit.body["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].clone())
        .collect();
    assert!(
        actions.contains(&json!("admin.mfa_enrolled")) && actions.contains(&json!("admin.login"))
    );
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn admin_api_is_invisible_outside_allowed_networks(pool: PgPool) {
    let app = App::configured(pool, |s| {
        s.admin_networks = Some(procure_server::admin_networks_from("10.0.0.0/8"));
    })
    .await;
    // In-process requests have no peer address: never on the allow-list.
    let r = post(
        &app,
        "/auth/login",
        None,
        json!({ "email": "x@y.z", "password": "p" }),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert!(procure_server::admin_networks_from("bogus, 192.168.1.0/24").len() == 1);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn organisations_rules_accounts_and_monitoring(pool: PgPool) {
    let w = world(pool).await;
    let app = &w.app;
    let (admin, _) = first_admin_login(app, "root@chuo-sogo.test").await;

    let orgs = app.get(&format!("{ADMIN}/orgs"), &admin).await;
    assert_eq!(orgs.body.as_array().unwrap().len(), 3);
    let a = orgs
        .body
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == json!(w.org_a))
        .unwrap()
        .clone();
    assert_eq!(a["members"], 1);
    let members = app
        .get(&format!("{ADMIN}/orgs/{}/members", w.org_a), &admin)
        .await;
    assert_eq!(members.body[0]["email"], "admin@a.test");

    // Rules: AI off for org A; identification SLA 48 h.
    let settings = format!("{ADMIN}/orgs/{}/settings", w.org_a);
    assert_eq!(
        app.get(&settings, &admin).await.body["identify_sla_hours"],
        24
    );
    let bad = app
        .put(
            &settings,
            &admin,
            json!({ "identify_sla_hours": 0, "chase_default_hours": 72,
        "ai_enabled": true, "default_tax_rate_bps": 1000 }),
        )
        .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    let r = app
        .put(
            &settings,
            &admin,
            json!({ "identify_sla_hours": 48, "chase_default_hours": 72,
        "ai_enabled": false, "default_tax_rate_bps": 1000 }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);
    let requester = w
        .member(w.org_a, &w.admin_a, "req@a.test", "requester")
        .await;
    let created = app.post(&format!("/api/v1/orgs/{}/requests", w.org_a), &requester,
        json!({ "title": "x", "currency": "JPY", "items": [{ "description": "a", "quantity": 1 }] })).await;
    let request = created.body["id"].as_str().unwrap().to_string();
    let r = app
        .post(
            &format!(
                "/api/v1/orgs/{}/requests/{request}/ai/extract-items",
                w.org_a
            ),
            &requester,
            json!({ "text": "1 x ABC123" }),
        )
        .await;
    assert_eq!(r.body["error"]["detail"], "ai_disabled");
    // The org's own admin sees the rule change in its audit log.
    let log = app
        .get(&format!("/api/v1/orgs/{}/audit", w.org_a), &w.admin_a)
        .await;
    assert!(log.body.to_string().contains("org.settings_updated"));

    // Suspending org A locks its members out of it; data stays.
    let r = app
        .patch(
            &format!("{ADMIN}/orgs/{}", w.org_a),
            &admin,
            json!({ "status": "suspended" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let me = app.get("/api/v1/me", &w.admin_a).await;
    assert_eq!(me.body["memberships"].as_array().unwrap().len(), 0);
    assert_eq!(
        app.get(&format!("/api/v1/orgs/{}/requests", w.org_a), &requester)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let r = app
        .patch(
            &format!("{ADMIN}/orgs/{}", w.platform_org),
            &admin,
            json!({ "status": "suspended" }),
        )
        .await;
    assert_eq!(r.body["error"]["detail"], "cannot_suspend_platform");
    let r = app
        .patch(
            &format!("{ADMIN}/orgs/{}", w.org_a),
            &admin,
            json!({ "status": "active", "timezone": "Mars/Base" }),
        )
        .await;
    assert_eq!(r.body["error"]["detail"], "timezone");
    app.patch(
        &format!("{ADMIN}/orgs/{}", w.org_a),
        &admin,
        json!({ "status": "active" }),
    )
    .await;

    // Accounts: search, deactivate (ends sessions), never yourself.
    let found = app.get(&format!("{ADMIN}/accounts?q=a.test"), &admin).await;
    let admin_a = found
        .body
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["email"] == "admin@a.test")
        .unwrap()
        .clone();
    let url = format!("{ADMIN}/accounts/{}", admin_a["id"].as_str().unwrap());
    assert_eq!(
        app.patch(&url, &admin, json!({ "status": "deactivated" }))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.get("/api/v1/me", &w.admin_a).await.status,
        StatusCode::UNAUTHORIZED
    );
    let me_id = app.get(&format!("{ADMIN}/me"), &admin).await.body["user"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = app
        .patch(
            &format!("{ADMIN}/accounts/{me_id}"),
            &admin,
            json!({ "status": "deactivated" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // Monitoring.
    let h = app.get(&format!("{ADMIN}/health"), &admin).await;
    assert_eq!(h.status, StatusCode::OK, "{:?}", h.body);
    assert_eq!(h.body["ai_provider"], "mock");
    assert!(h.body["schema_version"].as_i64().unwrap() >= 8);
    assert!(
        h.body["orgs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|o| o["audit_chain_valid"] == true)
    );

    // A procurement operator can sign in but has no administration rights.
    let op = w
        .member(
            w.platform_org,
            &w.super_admin,
            "op@chuo-sogo.test",
            "procurement_operator",
        )
        .await;
    let _ = op;
    let (op_admin, _) = first_admin_login(app, "op@chuo-sogo.test").await;
    assert_eq!(
        app.get(&format!("{ADMIN}/orgs"), &op_admin).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.get(&format!("{ADMIN}/health"), &op_admin).await.status,
        StatusCode::FORBIDDEN
    );
    // Resetting someone's authenticator ends their admin sessions; they enrol again.
    let op_id = app.get(&format!("{ADMIN}/me"), &op_admin).await.body["user"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = app
        .patch(
            &format!("{ADMIN}/accounts/{op_id}"),
            &admin,
            json!({ "reset_mfa": true }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(
        app.get(&format!("{ADMIN}/me"), &op_admin).await.status,
        StatusCode::UNAUTHORIZED
    );
    let r = post(
        app,
        "/auth/login",
        None,
        json!({ "email": "op@chuo-sogo.test", "password": PASSWORD }),
    )
    .await;
    assert_eq!(r.body["step"], "enroll");
}
