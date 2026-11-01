//! Phase E acceptance: consumer sign-up, shop rules and seller notice, ordering at
//! tax-inclusive prices, confirmation with a tax-inclusive invoice, payment, shipping,
//! cancellation with a credit note, and isolation between consumers.

// Test helpers: a failed unwrap is a failed test, which is the intent.
#![allow(clippy::unwrap_used)]

mod common;

use axum::http::{Method, StatusCode};
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

async fn anon(app: &App, method: Method, path: &str, body: Option<Value>) -> Resp {
    app.call(method, path, None, body).await
}

async fn admin_post(app: &App, path: &str, body: Value) -> Resp {
    anon(app, Method::POST, &format!("{ADMIN}{path}"), Some(body)).await
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

/// A shop-staff action on an order.
async fn act(s: &Shop, id: Uuid, action: Value) -> Resp {
    s.w.app
        .post(
            &format!("{ADMIN}/shop/orders/{id}/action"),
            &s.admin,
            action,
        )
        .await
}

fn jpeg() -> Vec<u8> {
    let seg = |marker: u8, payload: &[u8]| {
        let mut v = vec![0xFF, marker];
        v.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
        v.extend_from_slice(payload);
        v
    };
    let mut j = vec![0xFF, 0xD8];
    j.extend(seg(0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0"));
    let mut sof = vec![8];
    sof.extend_from_slice(&1200u16.to_be_bytes());
    sof.extend_from_slice(&1600u16.to_be_bytes());
    sof.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
    j.extend(seg(0xC0, &sof));
    j.extend([0xFF, 0xDA, 0x00, 0x02, 0x00, 0x00, 0xFF, 0xD9]);
    j
}

fn notice(open: bool) -> Value {
    json!({
        "open": open, "shipping_fee_minor": 880, "free_shipping_over_minor": 10000, "payment_days": 7,
        "seller_name": "株式会社中央総合", "responsible_person": "代表取締役 山田太郎",
        "seller_address": "大阪府大阪市北区梅田1-1-1", "seller_phone": "06-0000-0000",
        "seller_email": "shop@chuo-sogo.test", "price_info": "表示価格は税込です。",
        "payment_info": "銀行振込（ご注文確定後7日以内）", "delivery_info": "入金確認後3営業日以内に発送",
        "returns_policy": "未使用品に限り到着後8日以内", "privacy_policy": "個人情報は配送とご連絡にのみ使用します。"
    })
}

struct Shop {
    w: World,
    admin: String,
    listing: Uuid,
}

/// Issuer profile, one approved listing at ¥1,100 incl. tax (stock 5).
async fn setup(pool: PgPool) -> Shop {
    let w = world(pool).await;
    let app = &w.app;
    let admin = admin_session(app, "root@chuo-sogo.test").await;
    app.put(
        &format!("{ADMIN}/billing/issuer"),
        &admin,
        json!({ "legal_name": "株式会社中央総合", "registration_number": "T7000012050002",
                "address": "大阪府大阪市北区梅田1-1-1", "bank_details": "テスト銀行 本店 普通 1234567",
                "payment_terms_days": 30, "tax_rounding": "floor", "number_prefix": "CS" }),
    )
    .await;
    let r = app
        .post(
            "/api/v1/platform/organisations",
            &w.super_admin,
            json!({ "name": "大阪ベアリング", "kind": "supplier", "admin_email": "s@b.test", "admin_name": "S" }),
        )
        .await;
    let sup_org = uuid(&r.body["org_id"]);
    let sup = app.activate("s@b.test").await;
    app.accept(sup_org, &sup).await;
    let base = format!("/api/v1/orgs/{sup_org}/listings");
    let id = uuid(
        &app.post(
            &base,
            &sup,
            json!({ "manufacturer": "NSK", "model_number": "6204-2RS", "condition": "new",
                    "title": "深溝玉軸受", "description": "在庫品", "business_price_minor": 800,
                    "consumer_price_minor": 1100, "tax_rate_bps": 1000, "stock": 5 }),
        )
        .await
        .body["id"],
    );
    for n in 0..2 {
        app.post(
            &format!("{base}/{id}/media"),
            &sup,
            json!({ "kind": "photo", "file_name": format!("{n}.jpg"),
                    "data_base64": base64::engine::general_purpose::STANDARD.encode(jpeg()) }),
        )
        .await;
    }
    app.post(&format!("{base}/{id}/submit"), &sup, json!({}))
        .await;
    let r = app
        .post(&format!("{ADMIN}/listings/{id}/approve"), &admin, json!({}))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);
    Shop {
        w,
        admin,
        listing: id,
    }
}

/// Signs up, activates from the emailed token, logs in: (session, personal org id).
async fn consumer(app: &App, email: &str) -> (String, Uuid) {
    let r = anon(
        app,
        Method::POST,
        "/api/v1/shop/signup",
        Some(json!({ "email": email, "display_name": "佐藤花子", "locale": "ja" })),
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.body);
    let token = app.last_token(email);
    assert_eq!(
        app.set_password(&token, PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );
    let login = app.login(email, PASSWORD).await;
    let m = &login.body["memberships"][0];
    assert_eq!(m["org_kind"], "consumer");
    assert_eq!(m["role"], "consumer");
    (
        login.body["token"].as_str().unwrap().to_string(),
        uuid(&m["org_id"]),
    )
}

fn order(listing: Uuid, qty: u32, expected: i64) -> Value {
    json!({
        "lines": [{ "listing_id": listing, "quantity": qty }],
        "ship_to": { "name": "佐藤花子", "postal_code": "〒100 0001", "prefecture": "東京都",
                     "address_line": "千代田区千代田1-1", "phone": "03-1234-5678" },
        "expected_total_minor": expected
    })
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn consumers_buy_by_bank_transfer(pool: PgPool) {
    let s = setup(pool).await;
    let app = &s.w.app;
    let info = anon(app, Method::GET, "/api/v1/shop/info", None).await;
    assert_eq!(info.status, StatusCode::OK);
    assert_eq!(info.body["open"], false);

    let (c, org) = consumer(app, "hanako@example.test").await;
    let shop = |rest: &str| format!("/api/v1/orgs/{org}/shop/{rest}");
    let products = app.get(&shop("products"), &c).await;
    assert_eq!(products.body.as_array().unwrap().len(), 1);
    assert_eq!(products.body[0]["consumer_price_minor"], 1100);
    let page = app.get(&shop(&format!("products/{}", s.listing)), &c).await;
    assert_eq!(
        page.body["business_price_minor"],
        Value::Null,
        "business price hidden"
    );

    // Closed shop: no orders. Opening needs the complete seller notice.
    let r = app
        .post(&shop("orders"), &c, order(s.listing, 2, 3080))
        .await;
    assert_eq!(r.body["error"]["detail"], "shop_closed");
    let mut partial = notice(true);
    partial["returns_policy"] = json!("");
    let r = app
        .put(&format!("{ADMIN}/shop/settings"), &s.admin, partial)
        .await;
    assert_eq!(r.body["error"]["detail"], "notice_incomplete");
    assert_eq!(
        app.put(&format!("{ADMIN}/shop/settings"), &s.admin, notice(true))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        anon(app, Method::GET, "/api/v1/shop/info", None).await.body["seller_name"],
        "株式会社中央総合"
    );

    // 2 × ¥1,100 + ¥880 shipping = ¥3,080, tax included (¥280).
    let r = app
        .post(&shop("orders"), &c, order(s.listing, 2, 3000))
        .await;
    assert_eq!(
        r.body["error"]["detail"], "price_changed",
        "{} {:?}",
        r.status, r.body
    );
    let r = app
        .post(&shop("orders"), &c, order(s.listing, 6, 7480))
        .await;
    assert_eq!(r.body["error"]["detail"], "insufficient_stock");
    let r = app
        .post(&shop("orders"), &c, order(s.listing, 2, 3080))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["status"], "placed");
    assert_eq!(r.body["tax_minor"], 280);
    assert_eq!(r.body["ship_to"]["postal_code"], "100-0001");
    let id = uuid(&r.body["id"]);
    // A second order (stock 5: 2 + 3 can both be confirmed).
    let big = app
        .post(&shop("orders"), &c, order(s.listing, 3, 3300 + 880))
        .await;
    assert_eq!(big.status, StatusCode::CREATED, "{:?}", big.body);

    // The consumer cannot mark anything paid; payment needs the invoice first.
    assert_eq!(
        act(&s, id, json!({ "action": "paid", "paid_on": "2026-11-01" }))
            .await
            .status,
        StatusCode::CONFLICT
    );
    let r = act(&s, id, json!({ "action": "confirm" })).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["status"], "confirmed");
    let invoice_number = r.body["invoice_number"].as_str().unwrap().to_string();
    assert!(invoice_number.starts_with("CS-"));
    assert!(
        app.mailer
            .sent
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.template == "shop_order_confirmed" && m.to == "hanako@example.test")
    );

    // The consumer sees the tax-inclusive invoice and its PDF, never a draft.
    let invoices = app
        .get(&format!("/api/v1/orgs/{org}/billing/invoices"), &c)
        .await;
    assert_eq!(invoices.status, StatusCode::OK, "{:?}", invoices.body);
    assert_eq!(invoices.body[0]["total_minor"], 3080);
    let inv_id = invoices.body[0]["id"].as_str().unwrap();
    let detail = app
        .get(&format!("/api/v1/orgs/{org}/billing/invoices/{inv_id}"), &c)
        .await;
    assert_eq!(detail.body["tax_minor"], 280);
    assert_eq!(detail.body["subtotal_minor"], 2800);
    let (status, ct, pdf) = app
        .get_raw(
            &format!("/api/v1/orgs/{org}/billing/invoices/{inv_id}/pdf"),
            &c,
        )
        .await;
    assert_eq!((status, ct.as_str()), (StatusCode::OK, "application/pdf"));
    assert!(pdf.starts_with(b"%PDF-"));
    // Too late for the consumer to cancel by themselves.
    assert_eq!(
        app.post(&shop(&format!("orders/{id}/cancel")), &c, json!({}))
            .await
            .status,
        StatusCode::CONFLICT
    );

    let r = act(
        &s,
        id,
        json!({ "action": "paid", "paid_on": chrono::Utc::now().date_naive() }),
    )
    .await;
    assert_eq!(r.body["status"], "paid", "{:?}", r.body);
    let inv = app
        .get(&format!("/api/v1/orgs/{org}/billing/invoices/{inv_id}"), &c)
        .await;
    assert_eq!(inv.body["status"], "paid");
    assert_eq!(
        act(&s, id, json!({ "action": "ship", "carrier": "ヤマト運輸" }))
            .await
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let r = act(
        &s,
        id,
        json!({ "action": "ship", "carrier": "ヤマト運輸", "tracking_number": "1234-5678-9012" }),
    )
    .await;
    assert_eq!(r.body["status"], "shipped");
    let r = act(&s, id, json!({ "action": "complete" })).await;
    assert_eq!(r.body["status"], "completed");
    let mine = app.get(&shop(&format!("orders/{id}")), &c).await;
    assert_eq!(mine.body["tracking_number"], "1234-5678-9012");
    assert_eq!(mine.body["contact_email"], Value::Null);

    // Cancelling a confirmed order issues a credit note for the whole invoice.
    let big_id = uuid(&big.body["id"]);
    app.post(
        &format!("{ADMIN}/shop/orders/{big_id}/action"),
        &s.admin,
        json!({ "action": "confirm" }),
    )
    .await;
    let r = app
        .post(
            &format!("{ADMIN}/shop/orders/{big_id}/action"),
            &s.admin,
            json!({ "action": "cancel", "reason": "在庫切れ" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["status"], "cancelled");
    assert!(
        r.body["credit_note_number"]
            .as_str()
            .unwrap()
            .starts_with("CS-CN-")
    );
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn consumers_are_isolated_and_signup_reveals_nothing(pool: PgPool) {
    let owner = pool.clone();
    let s = setup(pool).await;
    let app = &s.w.app;
    app.put(&format!("{ADMIN}/shop/settings"), &s.admin, notice(true))
        .await;
    let (a, org_a) = consumer(app, "a@example.test").await;
    let (b, org_b) = consumer(app, "b@example.test").await;
    let placed = app
        .post(
            &format!("/api/v1/orgs/{org_a}/shop/orders"),
            &a,
            order(s.listing, 1, 1980),
        )
        .await;
    let id = uuid(&placed.body["id"]);
    // Another consumer sees nothing, through either account path.
    assert_eq!(
        app.get(&format!("/api/v1/orgs/{org_b}/shop/orders/{id}"), &b)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.get(&format!("/api/v1/orgs/{org_a}/shop/orders"), &b)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    // Business accounts have no shop; consumers have no business functions.
    assert_eq!(
        app.get(
            &format!("/api/v1/orgs/{}/shop/products", s.w.org_a),
            &s.w.admin_a
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.get(&format!("/api/v1/orgs/{org_a}/catalogue"), &a)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    // Signing up again with a known address: same answer, no mail.
    let before = app.mailer.sent.lock().unwrap().len();
    let r = anon(
        app,
        Method::POST,
        "/api/v1/shop/signup",
        Some(json!({ "email": "a@example.test", "display_name": "x" })),
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    assert_eq!(app.mailer.sent.lock().unwrap().len(), before);
    // The database refuses a consumer-side change other than withdrawing.
    let mut tx = owner.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.org_id', $1, true)")
        .bind(org_a.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let err = sqlx::query("UPDATE shop_orders SET status = 'confirmed' WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("cannot move"), "{err}");
    drop(tx);
    // The consumer can withdraw a placed order.
    assert_eq!(
        app.post(
            &format!("/api/v1/orgs/{org_a}/shop/orders/{id}/cancel"),
            &a,
            json!({ "reason": "間違えました" })
        )
        .await
        .status,
        StatusCode::NO_CONTENT
    );
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn review_regressions_stock_privacy_and_signup(pool: PgPool) {
    let s = setup(pool).await;
    let app = &s.w.app;
    app.put(&format!("{ADMIN}/shop/settings"), &s.admin, notice(true))
        .await;

    // An unactivated sign-up can be repeated: a fresh code goes to the real owner.
    anon(
        app,
        Method::POST,
        "/api/v1/shop/signup",
        Some(json!({ "email": "late@example.test", "display_name": "x" })),
    )
    .await;
    let first = app.last_token("late@example.test");
    anon(
        app,
        Method::POST,
        "/api/v1/shop/signup",
        Some(json!({ "email": "late@example.test", "display_name": "y" })),
    )
    .await;
    assert_ne!(app.last_token("late@example.test"), first);

    let (c, org) = consumer(app, "stock@example.test").await;
    // The product list shows neither business prices nor the supplier.
    let list = app
        .get(&format!("/api/v1/orgs/{org}/shop/products"), &c)
        .await;
    assert_eq!(list.body[0]["business_price_minor"], Value::Null);
    assert_eq!(list.body[0]["supplier_name"], "");

    // Two orders of 3 from a stock of 5: both can be placed, only one confirmed.
    let orders = format!("/api/v1/orgs/{org}/shop/orders");
    let a = uuid(&app.post(&orders, &c, order(s.listing, 3, 4180)).await.body["id"]);
    let b = uuid(&app.post(&orders, &c, order(s.listing, 3, 4180)).await.body["id"]);
    assert_eq!(
        act(&s, a, json!({ "action": "confirm" })).await.status,
        StatusCode::OK
    );
    let page = format!("/api/v1/orgs/{org}/shop/products/{}", s.listing);
    assert_eq!(app.get(&page, &c).await.body["stock"], 2);
    let r = act(&s, b, json!({ "action": "confirm" })).await;
    assert_eq!(r.body["error"]["detail"], "insufficient_stock");
    // Cancelling the confirmed order returns its stock; now the other one fits.
    act(&s, a, json!({ "action": "cancel", "reason": "顧客都合" })).await;
    assert_eq!(app.get(&page, &c).await.body["stock"], 5);
    assert_eq!(
        act(&s, b, json!({ "action": "confirm" })).await.status,
        StatusCode::OK
    );
    assert_eq!(app.get(&page, &c).await.body["stock"], 2);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn consumers_can_export_their_data_and_close_the_account(pool: PgPool) {
    let s = setup(pool).await;
    let app = &s.w.app;
    app.put(&format!("{ADMIN}/shop/settings"), &s.admin, notice(true))
        .await;
    let (c, org) = consumer(app, "privacy@example.test").await;
    let placed = app
        .post(
            &format!("/api/v1/orgs/{org}/shop/orders"),
            &c,
            order(s.listing, 1, 1980),
        )
        .await;
    let id = uuid(&placed.body["id"]);
    let data = app
        .get(&format!("/api/v1/orgs/{org}/shop/my-data"), &c)
        .await;
    assert_eq!(data.body["email"], "privacy@example.test");
    assert_eq!(data.body["orders"].as_array().unwrap().len(), 1);
    // Not while an order is open.
    let close = format!("/api/v1/orgs/{org}/shop/close-account");
    assert_eq!(
        app.post(&close, &c, json!({ "confirm": "DELETE" }))
            .await
            .body["error"]["detail"],
        "open_orders"
    );
    app.post(
        &format!("/api/v1/orgs/{org}/shop/orders/{id}/cancel"),
        &c,
        json!({}),
    )
    .await;
    assert_eq!(
        app.post(&close, &c, json!({ "confirm": "yes" }))
            .await
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        app.post(&close, &c, json!({ "confirm": "DELETE" }))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    // Signed out everywhere; the address and password no longer work; the address is free.
    assert_eq!(
        app.get("/api/v1/me", &c).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.login("privacy@example.test", PASSWORD).await.status,
        StatusCode::UNAUTHORIZED
    );
    // The order stays on record for staff (tax retention), without the person's account.
    let o = app
        .get(&format!("{ADMIN}/shop/orders/{id}"), &s.admin)
        .await;
    assert_eq!(o.body["status"], "cancelled");
    assert!(
        o.body["contact_email"]
            .as_str()
            .unwrap()
            .starts_with("closed-")
    );
}
