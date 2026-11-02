//! Items written differently for the same thing: proposals by likeness, confirmed by a person,
//! then used by the price reference. Permissions, the one-level rule, audit, tenant isolation.

#![allow(clippy::unwrap_used)]

mod common;

use axum::http::StatusCode;
use base64::Engine;
use common::*;
use serde_json::{Value, json};
use sqlx::PgPool;

const RECORDS: &str = "伝票日付,仕入先名,品名,型番,数量,単価（円）\n\
2025/05/01,山田商事,深溝玉軸受,6204ZZ,10,320\n\
2025/06/01,山田商事,深溝玉軸受,6204ZZ,10,330\n\
2025/07/01,東和精機,六角ボルト M8x30 ステンレス,,100,12\n\
2025/08/01,東和精機,ナット,N-M8,100,3\n";

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn confirmed_aliases_join_the_price_reference(pool: PgPool) {
    let w = world(pool).await;
    let app = &w.app;
    let url = |p: &str| format!("/api/v1/orgs/{}/{p}", w.org_a);
    let r = app
        .post(
            &url("purchase-history/imports"),
            &w.admin_a,
            json!({ "file_name": "records.csv",
                    "data_base64": base64::engine::general_purpose::STANDARD.encode(RECORDS) }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let r = app
        .post(
            &url("requests"),
            &w.admin_a,
            json!({ "title": "補充", "currency": "JPY", "items": [
                { "description": "ベアリング", "mpn": "6204-2Z", "quantity": 5 },
                { "description": "六角ボルト M8×30", "quantity": 50 },
                { "description": "Never bought", "mpn": "XYZ-1", "quantity": 1 }
            ]}),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let request = uuid(&r.body["id"]);
    let reference = url(&format!("requests/{request}/price-reference"));
    let refs = app.get(&reference, &w.admin_a).await.body;
    assert_eq!(refs[0]["item_key"], "m:62042Z");
    assert_eq!(refs[0]["summary"], Value::Null);
    let item = |n: usize| refs[n]["item_id"].as_str().unwrap().to_string();

    // Proposals: the bearing variant and the bolt description; nothing for an unknown part.
    let similar = |n: usize| {
        url(&format!(
            "requests/{request}/items/{}/similar-items",
            item(n)
        ))
    };
    let r = app.get(&similar(0), &w.admin_a).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body[0]["item_key"], "m:6204ZZ");
    assert_eq!(r.body[0]["purchases"], 2);
    assert_eq!(r.body[0]["last_unit_price_milli"], 330_000);
    assert!(r.body[0]["score"].as_u64().unwrap() >= 600);
    let r = app.get(&similar(1), &w.admin_a).await;
    assert_eq!(
        r.body[0]["item_key"], "d:六角ボルト m8x30 ステンレス",
        "{:?}",
        r.body
    );
    let r = app.get(&similar(2), &w.admin_a).await;
    assert_eq!(r.body, json!([]));

    // Nothing is linked until a person confirms it.
    let r = app
        .post(
            &url("item-aliases"),
            &w.admin_a,
            json!({ "alias_key": "m:62042Z", "canonical_key": "m:6204ZZ", "note": "同等品" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let alias = uuid(&r.body["id"]);
    let refs = app.get(&reference, &w.admin_a).await.body;
    assert_eq!(refs[0]["matched_on"], "alias");
    assert_eq!(refs[0]["summary"]["purchases"], 2);
    assert_eq!(refs[0]["summary"]["last"]["unit_price_milli"], 330_000);
    // Once linked, the proposal no longer lists the item.
    let r = app.get(&similar(0), &w.admin_a).await;
    assert!(
        r.body
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["item_key"] != "m:6204ZZ")
    );

    // Linking to an alias goes to its canonical key; chains and duplicates are refused.
    let r = app
        .post(
            &url("item-aliases"),
            &w.admin_a,
            json!({ "alias_key": "m:6204Z", "canonical_key": "m:62042Z" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["canonical_key"], "m:6204ZZ");
    for (body, status, detail) in [
        (
            json!({ "alias_key": "m:62042Z", "canonical_key": "m:NM8" }),
            StatusCode::CONFLICT,
            "alias_exists",
        ),
        (
            json!({ "alias_key": "m:6204ZZ", "canonical_key": "m:NM8" }),
            StatusCode::CONFLICT,
            "alias_chain",
        ),
        (
            json!({ "alias_key": "m:NM8", "canonical_key": "m:NM8" }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "canonical_key",
        ),
        (
            json!({ "alias_key": "x:1", "canonical_key": "m:NM8" }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "alias_key",
        ),
    ] {
        let r = app.post(&url("item-aliases"), &w.admin_a, body).await;
        assert_eq!(r.status, status, "{:?}", r.body);
        assert_eq!(r.body["error"]["detail"], detail);
    }

    let r = app.get(&url("item-aliases"), &w.admin_a).await;
    assert_eq!(r.body.as_array().unwrap().len(), 2);
    assert_eq!(r.body[1]["note"], "同等品");

    // Requesters may not see or change them; another organisation sees nothing.
    let requester = w
        .member(w.org_a, &w.admin_a, "req@a.example", "requester")
        .await;
    assert_eq!(
        app.get(&url("item-aliases"), &requester).await.status,
        StatusCode::FORBIDDEN
    );
    let r = app
        .post(
            &url("item-aliases"),
            &requester,
            json!({ "alias_key": "m:A1", "canonical_key": "m:B1" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let other = format!("/api/v1/orgs/{}/item-aliases", w.org_b);
    assert_eq!(app.get(&other, &w.admin_b).await.body, json!([]));
    let r = app.delete(&format!("{other}/{alias}"), &w.admin_b).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    // Removing the link removes the match; both changes are audited.
    let r = app
        .delete(&url(&format!("item-aliases/{alias}")), &w.admin_a)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let refs = app.get(&reference, &w.admin_a).await.body;
    assert_eq!(refs[0]["summary"], Value::Null);
    let audit = app.get(&url("audit?limit=100"), &w.admin_a).await.body;
    let actions: Vec<&str> = audit["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["action"].as_str())
        .collect();
    assert_eq!(
        actions
            .iter()
            .filter(|a| **a == "item_alias.created")
            .count(),
        2
    );
    assert!(actions.contains(&"item_alias.deleted"));
}
