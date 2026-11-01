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
    let app = App::configured(pool, |s| s.shop_signup = false).await;
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
