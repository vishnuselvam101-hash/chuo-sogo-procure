//! Safeguards for a publicly reachable server (the trial demo): the platform super-admin
//! cannot sign in to the apps from outside the admin networks, each organisation has a file
//! storage quota, and consumer self sign-up can be switched off.

// Test helpers: a failed unwrap is a failed test, which is the intent.
#![allow(clippy::unwrap_used)]

mod common;

use axum::http::{Method, StatusCode};
use base64::Engine;
use common::*;
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn super_admin_app_sign_in_only_from_admin_networks(pool: PgPool) {
    let _w = world(pool.clone()).await;
    // In-process requests have no peer address, so they count as outside any network.
    let outside = App::configured(pool, |s| {
        s.admin_networks = Some(procure_server::admin_networks_from("10.0.0.0/8"));
    })
    .await;
    let r = outside.login("root@chuo-sogo.test", PASSWORD).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{:?}", r.body);
    assert!(r.body.get("token").is_none());
    // Everyone else signs in as usual.
    let r = outside.login("admin@a.test", PASSWORD).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn uploads_stop_at_the_organisation_quota(pool: PgPool) {
    let app = App::configured(pool, |s| s.org_storage_quota = Some(100)).await;
    let w = world_with(app).await;
    let requester = w
        .member(w.org_a, &w.admin_a, "req@a.test", "requester")
        .await;
    let r = w
        .app
        .post(
            &format!("/api/v1/orgs/{}/requests", w.org_a),
            &requester,
            json!({ "title": "Quota", "currency": "JPY",
                    "items": [{ "description": "Bracket", "quantity": 1 }] }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let list = format!(
        "/api/v1/orgs/{}/requests/{}/attachments",
        w.org_a,
        uuid(&r.body["id"])
    );
    let pdf = |n: usize| {
        let mut b = b"%PDF-1.7\n".to_vec();
        b.resize(n, b' ');
        base64::engine::general_purpose::STANDARD.encode(b)
    };
    let r = w
        .app
        .post(
            &list,
            &requester,
            json!({ "file_name": "a.pdf", "data_base64": pdf(60) }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    // 60 + 60 bytes would exceed the 100-byte quota.
    let r = w
        .app
        .post(
            &list,
            &requester,
            json!({ "file_name": "b.pdf", "data_base64": pdf(60) }),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", r.body);
    assert_eq!(r.body["error"]["detail"], "storage_quota", "{:?}", r.body);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn shop_sign_up_can_be_switched_off(pool: PgPool) {
    // In-process requests have no peer address, so they count as outside the admin networks.
    let app = App::configured(pool, |s| {
        s.shop_signup = false;
        s.admin_networks = Some(procure_server::admin_networks_from("10.0.0.0/8"));
    })
    .await;
    let r = app
        .call(
            Method::POST,
            "/api/v1/shop/signup",
            None,
            Some(json!({ "email": "new@example.test", "display_name": "x" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{:?}", r.body);
    assert!(app.mailer.sent.lock().unwrap().is_empty());
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn trial_accounts_enter_without_password_only_where_configured(pool: PgPool) {
    // Off by default: no list, no entry.
    let w = world(pool.clone()).await;
    let r = w
        .app
        .call(Method::GET, "/api/v1/demo/accounts", None, None)
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = w
        .app
        .call(
            Method::POST,
            "/api/v1/auth/demo-login",
            None,
            Some(json!({ "email": "admin@a.test" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    // Trial server: the super-admin is listed by mistake and must still be refused.
    let trial = App::configured(pool, |s| {
        s.demo_accounts = vec!["admin@a.test".into(), "root@chuo-sogo.test".into()];
    })
    .await;
    let r = trial
        .call(Method::GET, "/api/v1/demo/accounts", None, None)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let listed: Vec<&str> = r
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["email"].as_str().unwrap())
        .collect();
    assert_eq!(listed, vec!["admin@a.test"]);
    assert_eq!(r.body[0]["role"], "customer_admin");
    assert_eq!(r.body[0]["org_kind"], "customer");

    let r = trial
        .call(
            Method::POST,
            "/api/v1/auth/demo-login",
            None,
            Some(json!({ "email": "Admin@A.test" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let token = r.body["token"].as_str().unwrap().to_string();
    let me = trial
        .call(Method::GET, "/api/v1/me", Some(&token), None)
        .await;
    assert_eq!(me.body["user"]["email"], "admin@a.test");

    for email in ["root@chuo-sogo.test", "nobody@a.test"] {
        let r = trial
            .call(
                Method::POST,
                "/api/v1/auth/demo-login",
                None,
                Some(json!({ "email": email })),
            )
            .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{email}: {:?}", r.body);
        assert!(r.body.get("token").is_none());
    }
}
