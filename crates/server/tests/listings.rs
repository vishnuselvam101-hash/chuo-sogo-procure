//! Phase C acceptance: supplier organisations, listings with photos (metadata removed),
//! moderation by Chuo-Sogo, and the catalogue customers see.

// Test helpers: a failed unwrap is a failed test, which is the intent.
#![allow(clippy::unwrap_used)]

mod common;

use axum::http::StatusCode;
use base64::Engine;
use common::*;
use procure_server::mfa;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

const ADMIN: &str = "/api/admin/v1";

fn code(secret_b32: &str) -> String {
    let secret = mfa::base32_decode(secret_b32).unwrap();
    let step = chrono::Utc::now().timestamp().div_euclid(mfa::STEP_SECONDS);
    format!("{:06}", mfa::code_at(&secret, step))
}

async fn admin_post(app: &App, path: &str, body: Value) -> Resp {
    app.call(
        axum::http::Method::POST,
        &format!("{ADMIN}{path}"),
        None,
        Some(body),
    )
    .await
}

async fn admin_session(app: &App, email: &str) -> String {
    let r = admin_post(
        app,
        "/auth/login",
        json!({ "email": email, "password": PASSWORD }),
    )
    .await;
    let token = r.body["token"].as_str().unwrap().to_string();
    let e = admin_post(app, "/auth/totp/enroll", json!({ "token": token })).await;
    let secret = e.body["secret"].as_str().unwrap().to_string();
    let c = admin_post(
        app,
        "/auth/totp/confirm",
        json!({ "token": token, "code": code(&secret) }),
    )
    .await;
    c.body["token"].as_str().unwrap().to_string()
}

/// A JPEG header of `w`×`h` with an EXIF segment carrying a GPS position (enough for the
/// server's checks; the pixels are not decoded).
fn jpeg(w: u16, h: u16) -> Vec<u8> {
    let seg = |marker: u8, payload: &[u8]| {
        let mut v = vec![0xFF, marker];
        v.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
        v.extend_from_slice(payload);
        v
    };
    let mut j = vec![0xFF, 0xD8];
    j.extend(seg(0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0"));
    j.extend(seg(0xE1, b"Exif\0\0GPS 34.70N 135.49E"));
    let mut sof = vec![8];
    sof.extend_from_slice(&h.to_be_bytes());
    sof.extend_from_slice(&w.to_be_bytes());
    sof.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
    j.extend(seg(0xC0, &sof));
    j.extend([0xFF, 0xDA, 0x00, 0x02, 0x00, 0x00, 0xFF, 0xD9]);
    j
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn listing() -> Value {
    json!({
        "manufacturer": "NSK",
        "model_number": "6204-2RS",
        "condition": "new",
        "title": "深溝玉軸受 6204-2RS",
        "title_en": "Deep groove ball bearing 6204-2RS",
        "description": "両側ゴムシール。在庫品。",
        "specs": [{ "key": "内径", "value": "20 mm" }, { "key": "外径", "value": "47 mm" }],
        "business_price_minor": 480,
        "tax_rate_bps": 1000,
        "stock": 120
    })
}

struct Cat {
    w: World,
    admin: String,
    supplier_org: Uuid,
    supplier: String,
}

async fn setup(pool: PgPool) -> Cat {
    let w = world(pool).await;
    let r = w
        .app
        .post(
            "/api/v1/platform/organisations",
            &w.super_admin,
            json!({ "name": "大阪ベアリング株式会社", "kind": "supplier",
                    "admin_email": "sales@bearing.test", "admin_name": "営業" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let supplier_org = uuid(&r.body["org_id"]);
    let supplier = w.app.activate("sales@bearing.test").await;
    w.app.accept(supplier_org, &supplier).await;
    let admin = admin_session(&w.app, "root@chuo-sogo.test").await;
    Cat {
        w,
        admin,
        supplier_org,
        supplier,
    }
}

impl Cat {
    fn app(&self) -> &App {
        &self.w.app
    }
    fn own(&self) -> String {
        format!("/api/v1/orgs/{}/listings", self.supplier_org)
    }
    async fn upload(&self, id: Uuid, kind: &str, name: &str, bytes: &[u8]) -> Resp {
        self.app()
            .post(
                &format!("{}/{id}/media", self.own()),
                &self.supplier,
                json!({ "kind": kind, "file_name": name, "data_base64": b64(bytes) }),
            )
            .await
    }
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn supplier_lists_chuo_sogo_reviews_customers_browse(pool: PgPool) {
    let c = setup(pool).await;
    let app = c.app();
    let r = app.post(&c.own(), &c.supplier, listing()).await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let id = uuid(&r.body["id"]);

    // Not complete yet: two photos are needed.
    let d = app.get(&format!("{}/{id}", c.own()), &c.supplier).await;
    assert_eq!(d.body["missing"], json!(["photos"]));
    let r = app
        .post(&format!("{}/{id}/submit", c.own()), &c.supplier, json!({}))
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(r.body["error"]["detail"], "listing_incomplete");

    // Too small, wrong type, or a renamed file: refused.
    let r = c.upload(id, "photo", "small.jpg", &jpeg(800, 600)).await;
    assert_eq!(r.body["error"]["detail"], "photo_too_small");
    let r = c.upload(id, "photo", "x.pdf", b"%PDF-1.7 fake").await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    let r = c
        .upload(id, "photo", "fake.jpg", b"MZ\x90\0 not an image")
        .await;
    assert_eq!(r.body["error"]["detail"], "file_type");

    for n in 0..2 {
        let r = c
            .upload(id, "photo", &format!("photo{n}.jpg"), &jpeg(1600, 1200))
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    }
    assert_eq!(
        c.upload(id, "document", "datasheet.pdf", b"%PDF-1.7\n...")
            .await
            .status,
        StatusCode::CREATED
    );
    let d = app.get(&format!("{}/{id}", c.own()), &c.supplier).await;
    assert_eq!(d.body["missing"], json!([]));
    let photo = d.body["media"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["kind"] == "photo")
        .unwrap()
        .clone();
    assert_eq!(photo["width"], 1600);
    // Stored without the GPS position.
    let (status, ct, bytes) = app
        .get_raw(
            &format!(
                "{}/{id}/media/{}/content",
                c.own(),
                photo["id"].as_str().unwrap()
            ),
            &c.supplier,
        )
        .await;
    assert_eq!((status, ct.as_str()), (StatusCode::OK, "image/jpeg"));
    assert!(!String::from_utf8_lossy(&bytes).contains("GPS"));

    // Not visible to customers before approval.
    let catalogue = format!("/api/v1/orgs/{}/catalogue", c.w.org_a);
    assert_eq!(app.get(&catalogue, &c.w.admin_a).await.body, json!([]));
    assert_eq!(
        app.get(&format!("{catalogue}/{id}"), &c.w.admin_a)
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        app.post(&format!("{}/{id}/submit", c.own()), &c.supplier, json!({}))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    // In the review queue; a rejection needs a reason.
    let q = app.get(&format!("{ADMIN}/listings"), &c.admin).await;
    assert_eq!(q.body.as_array().unwrap().len(), 1);
    assert_eq!(q.body[0]["supplier_name"], "大阪ベアリング株式会社");
    let r = app
        .post(
            &format!("{ADMIN}/listings/{id}/reject"),
            &c.admin,
            json!({ "note": "" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    app.post(
        &format!("{ADMIN}/listings/{id}/reject"),
        &c.admin,
        json!({ "note": "写真に型番の銘板を含めてください" }),
    )
    .await;
    let d = app.get(&format!("{}/{id}", c.own()), &c.supplier).await;
    assert_eq!(d.body["status"], "rejected");
    assert_eq!(d.body["review_note"], "写真に型番の銘板を含めてください");

    // Fix, resubmit, approve: now in the catalogue with the supplier's name.
    app.put(&format!("{}/{id}", c.own()), &c.supplier, listing())
        .await;
    app.post(&format!("{}/{id}/submit", c.own()), &c.supplier, json!({}))
        .await;
    assert_eq!(
        app.post(
            &format!("{ADMIN}/listings/{id}/approve"),
            &c.admin,
            json!({})
        )
        .await
        .status,
        StatusCode::NO_CONTENT
    );
    let list = app
        .get(&format!("{catalogue}?q=6204%202rs"), &c.w.admin_a)
        .await;
    assert_eq!(list.status, StatusCode::OK, "{:?}", list.body);
    assert_eq!(list.body.as_array().unwrap().len(), 1);
    assert_eq!(list.body[0]["supplier_name"], "大阪ベアリング株式会社");
    assert_eq!(
        app.get(&format!("{catalogue}?q=100%25"), &c.w.admin_a)
            .await
            .body,
        json!([])
    );
    let item = app.get(&format!("{catalogue}/{id}"), &c.w.admin_a).await;
    assert_eq!(item.body["specs"][0]["value"], "20 mm");
    assert_eq!(item.body["review_note"], "");
    let (status, _, _) = app
        .get_raw(
            &format!(
                "{catalogue}/{id}/media/{}/content",
                photo["id"].as_str().unwrap()
            ),
            &c.w.admin_a,
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Any change unpublishes it until reviewed again.
    let mut changed = listing();
    changed["business_price_minor"] = json!(1);
    app.put(&format!("{}/{id}", c.own()), &c.supplier, changed)
        .await;
    assert_eq!(app.get(&catalogue, &c.w.admin_a).await.body, json!([]));

    // Audit trail in the supplier's organisation.
    let audit = app
        .get(
            &format!("/api/v1/orgs/{}/audit?limit=50", c.supplier_org),
            &c.supplier,
        )
        .await;
    let actions: Vec<&str> = audit.body["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    for a in [
        "listing.created",
        "listing.media_added",
        "listing.submitted",
        "listing.rejected",
        "listing.approved",
        "listing.updated",
    ] {
        assert!(actions.contains(&a), "{a} missing");
    }
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn roles_and_tenants_stay_separate(pool: PgPool) {
    let c = setup(pool).await;
    let app = c.app();
    let id = uuid(&app.post(&c.own(), &c.supplier, listing()).await.body["id"]);

    // Customers cannot write listings, nor read another company's drafts.
    assert_eq!(
        app.post(
            &format!("/api/v1/orgs/{}/listings", c.w.org_a),
            &c.w.admin_a,
            listing()
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.get(&format!("{}/{id}", c.own()), &c.w.admin_a)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    // Suppliers cannot request procurement, browse others, or moderate.
    assert_eq!(
        app.get(
            &format!("/api/v1/orgs/{}/requests", c.supplier_org),
            &c.supplier
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.get(
            &format!("/api/v1/orgs/{}/catalogue", c.supplier_org),
            &c.supplier
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.post(
            &format!("{ADMIN}/listings/{id}/approve"),
            &c.supplier,
            json!({})
        )
        .await
        .status,
        StatusCode::UNAUTHORIZED
    );
    // A second supplier sees only its own listings.
    let r = app
        .post(
            "/api/v1/platform/organisations",
            &c.w.super_admin,
            json!({ "name": "Other Parts", "kind": "supplier",
                    "admin_email": "other@parts.test", "admin_name": "O" }),
        )
        .await;
    let other_org = uuid(&r.body["org_id"]);
    let other = app.activate("other@parts.test").await;
    app.accept(other_org, &other).await;
    assert_eq!(
        app.get(&format!("{}/{id}", c.own()), &other).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.get(&format!("/api/v1/orgs/{other_org}/listings"), &other)
            .await
            .body,
        json!([])
    );
    // Supplier roles only exist in supplier organisations.
    let r = app
        .post(
            &format!("/api/v1/orgs/{}/members", c.w.org_a),
            &c.w.admin_a,
            json!({ "email": "x@a.test", "display_name": "x", "role": "supplier_staff" }),
        )
        .await;
    assert_eq!(r.body["error"]["detail"], "role_not_allowed_in_org");
    let r = app
        .post(
            &format!("/api/v1/orgs/{}/members", c.supplier_org),
            &c.supplier,
            json!({ "email": "staff@bearing.test", "display_name": "staff", "role": "supplier_staff" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);

    // Retired listings are final; drafts never reviewed can be deleted.
    let draft = uuid(&app.post(&c.own(), &c.supplier, listing()).await.body["id"]);
    assert_eq!(
        app.delete(&format!("{}/{draft}", c.own()), &c.supplier)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    app.post(&format!("{}/{id}/retire", c.own()), &c.supplier, json!({}))
        .await;
    assert_eq!(
        app.put(&format!("{}/{id}", c.own()), &c.supplier, listing())
            .await
            .status,
        StatusCode::CONFLICT
    );
    // A serial number identifies a single unit.
    let mut one = listing();
    one["serial_number"] = json!("SN-0001");
    one["condition"] = json!("used");
    assert_eq!(
        app.post(&c.own(), &c.supplier, one.clone()).await.body["error"]["detail"],
        "stock"
    );
    one["stock"] = json!(1);
    assert_eq!(
        app.post(&c.own(), &c.supplier, one).await.status,
        StatusCode::CREATED
    );
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn moderators_see_possible_duplicates(pool: PgPool) {
    let c = setup(pool).await;
    let app = c.app();
    let mut ids = vec![];
    for model in ["6204-2RS", "6204 2rs"] {
        let mut l = listing();
        l["model_number"] = json!(model);
        let id = uuid(&app.post(&c.own(), &c.supplier, l).await.body["id"]);
        for n in 0..2 {
            c.upload(id, "photo", &format!("p{n}.jpg"), &jpeg(1200, 1200))
                .await;
        }
        app.post(&format!("{}/{id}/submit", c.own()), &c.supplier, json!({}))
            .await;
        ids.push(id);
    }
    let d = app
        .get(&format!("{ADMIN}/listings/{}", ids[1]), &c.admin)
        .await;
    assert_eq!(d.status, StatusCode::OK, "{:?}", d.body);
    let dups = d.body["duplicates"].as_array().unwrap();
    assert_eq!(dups.len(), 1);
    assert_eq!(uuid(&dups[0]["id"]), ids[0]);
    // Suppliers never see the duplicate check.
    let own = app
        .get(&format!("{}/{}", c.own(), ids[1]), &c.supplier)
        .await;
    assert_eq!(own.body["duplicates"], json!([]));
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn review_regressions_slots_suspension_and_database_guard(pool: PgPool) {
    let owner = pool.clone();
    let c = setup(pool).await;
    let app = c.app();
    let id = uuid(&app.post(&c.own(), &c.supplier, listing()).await.body["id"]);
    // Delete-and-add many times: photo slots are reused, never exhausted.
    let mut photo_ids = vec![];
    for n in 0..5 {
        let r = c
            .upload(id, "photo", &format!("p{n}.jpg"), &jpeg(1600, 1200))
            .await;
        photo_ids.push(uuid(&r.body["id"]));
    }
    assert_eq!(
        c.upload(id, "photo", "p6.jpg", &jpeg(1600, 1200))
            .await
            .body["error"]["detail"],
        "too_many_files"
    );
    for round in 0..8 {
        let gone = photo_ids.remove(0);
        app.delete(&format!("{}/{id}/media/{gone}", c.own()), &c.supplier)
            .await;
        let r = c
            .upload(id, "photo", &format!("r{round}.jpg"), &jpeg(1600, 1200))
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "round {round}: {:?}", r.body);
        photo_ids.push(uuid(&r.body["id"]));
    }
    // A header claiming an enormous image is refused.
    assert_eq!(
        c.upload(id, "photo", "bomb.jpg", &jpeg(60_000, 15_000))
            .await
            .body["error"]["detail"],
        "photo_too_small"
    );

    app.post(&format!("{}/{id}/submit", c.own()), &c.supplier, json!({}))
        .await;
    app.post(
        &format!("{ADMIN}/listings/{id}/approve"),
        &c.admin,
        json!({}),
    )
    .await;
    let catalogue = format!("/api/v1/orgs/{}/catalogue", c.w.org_a);
    assert_eq!(
        app.get(&catalogue, &c.w.admin_a)
            .await
            .body
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // The database refuses a supplier-side transaction that publishes or reviews by itself.
    let mut tx = owner.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.org_id', $1, true)")
        .bind(c.supplier_org.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE product_listings SET status = 'draft' WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let err = sqlx::query("UPDATE product_listings SET status = 'approved' WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("supplier cannot move"), "{err}");
    drop(tx);

    // Suspending the supplier hides its listings; reactivating shows them again.
    let org_path = format!("{ADMIN}/orgs/{}", c.supplier_org);
    app.patch(&org_path, &c.admin, json!({ "status": "suspended" }))
        .await;
    assert_eq!(app.get(&catalogue, &c.w.admin_a).await.body, json!([]));
    assert_eq!(
        app.get(&format!("{catalogue}/{id}"), &c.w.admin_a)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    app.patch(&org_path, &c.admin, json!({ "status": "active" }))
        .await;
    assert_eq!(
        app.get(&catalogue, &c.w.admin_a)
            .await
            .body
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
