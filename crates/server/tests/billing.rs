//! Phase B acceptance: invoices Chuo-Sogo issues (qualified invoices) and credit notes —
//! per-rate tax, gap-free numbers, immutability (API and database), PDFs, payments, and
//! what customers can see.

// Test helpers: a failed unwrap is a failed test, which is the intent.
#![allow(clippy::unwrap_used)]

mod common;

use axum::http::StatusCode;
use chrono::Datelike;
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

/// Admin console session for an internal account (first sign-in with enrolment).
async fn admin_session(app: &App, email: &str) -> String {
    let r = admin_post(
        app,
        "/auth/login",
        json!({ "email": email, "password": PASSWORD }),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let token = r.body["token"].as_str().unwrap().to_string();
    let e = admin_post(app, "/auth/totp/enroll", json!({ "token": token })).await;
    let secret = e.body["secret"].as_str().unwrap().to_string();
    let c = admin_post(
        app,
        "/auth/totp/confirm",
        json!({ "token": token, "code": code(&secret) }),
    )
    .await;
    assert_eq!(c.status, StatusCode::OK, "{:?}", c.body);
    c.body["token"].as_str().unwrap().to_string()
}

/// Runs `sql` (one `$1` = id) as the table owner inside a platform-scoped transaction for
/// `org`, i.e. past every application check and row-level policy: only the triggers remain.
/// Ok only if a row was actually changed.
async fn tamper(owner: &PgPool, org: Uuid, sql: &str, id: Uuid) -> Result<(), String> {
    let mut tx = owner.begin().await.unwrap();
    sqlx::query(
        "SELECT set_config('app.scope', 'platform', true), set_config('app.org_id', $1, true)",
    )
    .bind(org.to_string())
    .execute(&mut *tx)
    .await
    .unwrap();
    let r = sqlx::query(sql)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    assert!(
        r.rows_affected() > 0,
        "row not visible: the test would prove nothing"
    );
    Ok(())
}

fn issuer() -> Value {
    json!({
        "legal_name": "株式会社中央総合",
        "registration_number": "T7000012050002",
        "address": "大阪府大阪市北区梅田1-1-1",
        "phone": "06-0000-0000",
        "email": "billing@chuo-sogo.test",
        "bank_details": "テスト銀行 本店 普通 1234567",
        "payment_terms_days": 30,
        "tax_rounding": "floor",
        "number_prefix": "CS"
    })
}

/// 3 × ¥333 at 10 % and 2 × ¥105 at 8 %: per-rate tax 99 + 16 (not per line: 99 + 16.8→16).
fn draft() -> Value {
    json!({
        "recipient_address": "東京都千代田区千代田1-1",
        "notes": "10月分",
        "lines": [
            { "description": "調達代行サービス料", "quantity": 3, "unit": "式", "unit_price_minor": 333, "tax_rate_bps": 1000 },
            { "description": "飲料（軽減税率）", "quantity": 2, "unit_price_minor": 105, "tax_rate_bps": 800 }
        ]
    })
}

struct Billing {
    w: World,
    admin: String,
    finance_a: String,
}

async fn setup(pool: PgPool) -> Billing {
    let w = world(pool).await;
    let admin = admin_session(&w.app, "root@chuo-sogo.test").await;
    let finance_a = w
        .member(w.org_a, &w.admin_a, "finance@a.test", "finance")
        .await;
    Billing {
        w,
        admin,
        finance_a,
    }
}

impl Billing {
    fn app(&self) -> &App {
        &self.w.app
    }
    fn docs(&self, org: Uuid) -> String {
        format!("{ADMIN}/orgs/{org}/billing/invoices")
    }
    async fn create(&self, org: Uuid, body: Value) -> Uuid {
        let r = self.app().post(&self.docs(org), &self.admin, body).await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        uuid(&r.body["id"])
    }
    async fn issue(&self, org: Uuid, id: Uuid) -> Resp {
        self.app()
            .post(
                &format!("{}/{id}/issue", self.docs(org)),
                &self.admin,
                json!({}),
            )
            .await
    }
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn issuer_profile_is_validated_and_restricted(pool: PgPool) {
    let b = setup(pool).await;
    let app = b.app();
    let path = format!("{ADMIN}/billing/issuer");
    assert_eq!(app.get(&path, &b.admin).await.status, StatusCode::NOT_FOUND);
    let mut bad = issuer();
    bad["registration_number"] = json!("T8000012050002"); // wrong check digit
    let r = app.put(&path, &b.admin, bad).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(r.body["error"]["detail"], "registration_number");
    let mut bad = issuer();
    bad["number_prefix"] = json!("cs-1");
    assert_eq!(
        app.put(&path, &b.admin, bad).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        app.put(&path, &b.admin, issuer()).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.get(&path, &b.admin).await.body["registration_number"],
        "T7000012050002"
    );

    // An operator (no manage_billing) cannot touch billing, even with an admin session.
    b.w.member(
        b.w.platform_org,
        &b.w.super_admin,
        "op@chuo-sogo.test",
        "procurement_operator",
    )
    .await;
    let op = admin_session(app, "op@chuo-sogo.test").await;
    assert_eq!(app.get(&path, &op).await.status, StatusCode::FORBIDDEN);
    assert_eq!(
        app.post(&b.docs(b.w.org_a), &op, draft()).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.get(&format!("{ADMIN}/billing/invoices"), &op)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn invoices_are_computed_per_rate_numbered_without_gaps_and_immutable(pool: PgPool) {
    let owner = pool.clone();
    let b = setup(pool).await;
    let app = b.app();
    let org = b.w.org_a;
    let id = b.create(org, draft()).await;
    let d = app.get(&format!("{}/{id}", b.docs(org)), &b.admin).await;
    assert_eq!(d.status, StatusCode::OK, "{:?}", d.body);
    assert_eq!(d.body["status"], "draft");
    assert_eq!(d.body["recipient_name"], "株式会社エー製作所");
    assert_eq!(d.body["subtotal_minor"], 1209);
    assert_eq!(d.body["tax_minor"], 99 + 16);
    assert_eq!(d.body["total_minor"], 1209 + 115);
    assert_eq!(d.body["per_rate"][0]["rate_bps"], 1000);
    assert_eq!(d.body["per_rate"][1]["tax_minor"], 16);

    // Bad drafts are refused.
    let mut bad = draft();
    bad["lines"][0]["tax_rate_bps"] = json!(500);
    assert_eq!(
        app.post(&b.docs(org), &b.admin, bad).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut bad = draft();
    bad["lines"] = json!([]);
    assert_eq!(
        app.post(&b.docs(org), &b.admin, bad).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // The platform organisation is not a customer.
    assert_eq!(
        app.post(&b.docs(b.w.platform_org), &b.admin, draft())
            .await
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // Not without an issuer profile (registration number).
    let r = b.issue(org, id).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.code(), "conflict");
    assert_eq!(r.body["error"]["detail"], "issuer_not_configured");
    app.put(&format!("{ADMIN}/billing/issuer"), &b.admin, issuer())
        .await;

    // A deleted draft never consumes a number.
    let scrap = b.create(org, draft()).await;
    assert_eq!(
        app.delete(&format!("{}/{scrap}", b.docs(org)), &b.admin)
            .await
            .status,
        StatusCode::NO_CONTENT
    );

    let issued = b.issue(org, id).await;
    assert_eq!(issued.status, StatusCode::OK, "{:?}", issued.body);
    let year = (chrono::Utc::now() + chrono::Duration::hours(9)).year();
    assert_eq!(issued.body["number"], format!("CS-{year}-00001"));
    assert_eq!(issued.body["status"], "issued");
    let issue_date: chrono::NaiveDate =
        issued.body["issue_date"].as_str().unwrap().parse().unwrap();
    let due: chrono::NaiveDate = issued.body["due_date"].as_str().unwrap().parse().unwrap();
    assert_eq!(due - issue_date, chrono::Duration::days(30));

    // The PDF is stored, served as PDF, and matches the hash recorded at issue.
    let (status, ct, pdf) = app
        .get_raw(&format!("{}/{id}/pdf", b.docs(org)), &b.admin)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, "application/pdf");
    assert!(pdf.starts_with(b"%PDF-"));
    let sha: String = procure_server::storage::sha256(&pdf)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(issued.body["pdf_sha256"], sha);

    // Issued means immutable: no second issue, no edit, no delete...
    assert_eq!(b.issue(org, id).await.status, StatusCode::CONFLICT);
    assert_eq!(
        app.put(&format!("{}/{id}", b.docs(org)), &b.admin, draft())
            .await
            .status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.delete(&format!("{}/{id}", b.docs(org)), &b.admin)
            .await
            .status,
        StatusCode::CONFLICT
    );
    // ...and the database itself refuses, even with full write access in the platform scope.
    let err = tamper(
        &owner,
        org,
        "UPDATE issued_invoices SET total_minor = 1 WHERE id = $1",
        id,
    )
    .await
    .unwrap_err();
    assert!(err.contains("immutable"), "{err}");
    assert!(
        tamper(
            &owner,
            org,
            "DELETE FROM issued_invoice_lines WHERE invoice_id = $1",
            id
        )
        .await
        .is_err()
    );
    assert!(
        tamper(&owner, org, "DELETE FROM issued_invoices WHERE id = $1", id)
            .await
            .is_err()
    );

    // The next invoice takes the next number.
    let second = b.create(b.w.org_b, draft()).await;
    let r = b.issue(b.w.org_b, second).await;
    assert_eq!(r.body["number"], format!("CS-{year}-00002"));

    // Payment: dates checked; status follows; recorded in the audit log.
    let pay = format!("{}/{id}/payment", b.docs(org));
    let tomorrow = issue_date + chrono::Duration::days(1);
    assert_eq!(
        app.post(&pay, &b.admin, json!({ "paid": true, "paid_on": tomorrow }))
            .await
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        app.post(
            &pay,
            &b.admin,
            json!({ "paid": true, "paid_on": issue_date, "note": "振込確認" })
        )
        .await
        .status,
        StatusCode::NO_CONTENT
    );
    let d = app.get(&format!("{}/{id}", b.docs(org)), &b.admin).await;
    assert_eq!(d.body["status"], "paid");
    assert_eq!(d.body["paid_note"], "振込確認");
    assert!(
        tamper(
            &owner,
            org,
            "UPDATE issued_invoices SET notes = 'changed' WHERE id = $1",
            id
        )
        .await
        .is_err()
    );
    let list = app
        .get(&format!("{ADMIN}/billing/invoices?status=paid"), &b.admin)
        .await;
    assert_eq!(list.body.as_array().unwrap().len(), 1);
    let audit = app
        .get(&format!("{ADMIN}/orgs/{org}/audit?limit=50"), &b.admin)
        .await;
    let actions: Vec<&str> = audit.body["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert!(actions.contains(&"billing.issued"));
    assert!(actions.contains(&"billing.paid"));
    assert_eq!(audit.body["chain_valid"], true);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn credit_notes_reference_the_original_and_never_exceed_it(pool: PgPool) {
    let b = setup(pool).await;
    let app = b.app();
    let org = b.w.org_a;
    app.put(&format!("{ADMIN}/billing/issuer"), &b.admin, issuer())
        .await;
    let id = b.create(org, draft()).await;
    let credits = format!("{}/{id}/credit-notes", b.docs(org));
    let credit = |price: i64, rate: u32| {
        json!({ "reason": "返品", "lines": [
            { "description": "返品分", "quantity": 1, "unit_price_minor": price, "tax_rate_bps": rate }
        ]})
    };
    // Not against a draft.
    assert_eq!(
        app.post(&credits, &b.admin, credit(100, 1000)).await.status,
        StatusCode::CONFLICT
    );
    b.issue(org, id).await;
    // More than the 10 % part (999), or a rate the invoice never charged: refused.
    let r = app.post(&credits, &b.admin, credit(1000, 1000)).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.body["error"]["detail"], "credit_exceeds_original");
    assert_eq!(
        app.post(&credits, &b.admin, credit(1, 0)).await.status,
        StatusCode::CONFLICT
    );
    // A reason is required.
    assert_eq!(
        app.post(
            &credits,
            &b.admin,
            json!({ "reason": " ", "lines": credit(1, 1000)["lines"] })
        )
        .await
        .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let cn = uuid(&app.post(&credits, &b.admin, credit(600, 1000)).await.body["id"]);
    let r = b.issue(org, cn).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let year = (chrono::Utc::now() + chrono::Duration::hours(9)).year();
    assert_eq!(r.body["number"], format!("CS-CN-{year}-00001"));
    assert_eq!(r.body["kind"], "credit_note");
    assert_eq!(r.body["total_minor"], 600 + 60);
    assert_eq!(r.body["due_date"], Value::Null);

    // Two drafts that each fit, but not together: the second fails at issue.
    let a = uuid(&app.post(&credits, &b.admin, credit(300, 1000)).await.body["id"]);
    let c = uuid(&app.post(&credits, &b.admin, credit(300, 1000)).await.body["id"]);
    assert_eq!(b.issue(org, a).await.status, StatusCode::OK);
    let r = b.issue(org, c).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.body["error"]["detail"], "credit_exceeds_original");

    let d = app.get(&format!("{}/{id}", b.docs(org)), &b.admin).await;
    assert_eq!(d.body["credit_notes"].as_array().unwrap().len(), 3);
    // Credit notes are never "paid".
    assert_eq!(
        app.post(
            &format!("{}/{cn}/payment", b.docs(org)),
            &b.admin,
            json!({ "paid": true, "paid_on": r.body["issue_date"] })
        )
        .await
        .status,
        StatusCode::CONFLICT
    );
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn customers_see_only_their_own_issued_documents(pool: PgPool) {
    let b = setup(pool).await;
    let app = b.app();
    let (org_a, org_b) = (b.w.org_a, b.w.org_b);
    app.put(&format!("{ADMIN}/billing/issuer"), &b.admin, issuer())
        .await;
    let draft_id = b.create(org_a, draft()).await;
    let issued_id = b.create(org_a, draft()).await;
    b.issue(org_a, issued_id).await;
    let b_doc = b.create(org_b, draft()).await;
    b.issue(org_b, b_doc).await;

    let list = |org: Uuid| format!("/api/v1/orgs/{org}/billing/invoices");
    let r = app.get(&list(org_a), &b.finance_a).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let ids: Vec<Uuid> = r
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|d| uuid(&d["id"]))
        .collect();
    assert_eq!(
        ids,
        vec![issued_id],
        "drafts and other customers stay hidden"
    );
    assert_eq!(
        app.get(&format!("{}/{draft_id}", list(org_a)), &b.finance_a)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    // Another customer's document, even through this customer's own path.
    assert_eq!(
        app.get(&format!("{}/{b_doc}", list(org_a)), &b.finance_a)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.get(&list(org_b), &b.finance_a).await.status,
        StatusCode::NOT_FOUND
    );
    let (status, ct, pdf) = app
        .get_raw(&format!("{}/{issued_id}/pdf", list(org_a)), &b.finance_a)
        .await;
    assert_eq!((status, ct.as_str()), (StatusCode::OK, "application/pdf"));
    assert!(pdf.starts_with(b"%PDF-"));
    assert_eq!(
        app.get_raw(&format!("{}/{draft_id}/pdf", list(org_a)), &b.finance_a)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    // Without view_finance (a requester), nothing.
    let requester =
        b.w.member(org_a, &b.w.admin_a, "req@a.test", "requester")
            .await;
    assert_eq!(
        app.get(&list(org_a), &requester).await.status,
        StatusCode::FORBIDDEN
    );
    // Customer sessions cannot reach the admin billing API at all.
    assert_eq!(
        app.get(&format!("{ADMIN}/billing/invoices"), &b.finance_a)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn review_regressions_tax_cap_payment_dates_recipient(pool: PgPool) {
    let b = setup(pool).await;
    let app = b.app();
    let org = b.w.org_a;
    // Round up, so separately rounded credit notes could return more tax than was charged.
    let mut ceil = issuer();
    ceil["tax_rounding"] = json!("ceil");
    app.put(&format!("{ADMIN}/billing/issuer"), &b.admin, ceil)
        .await;
    let one_line = |price: i64| {
        json!({ "recipient_name": "エー製作所 経理部 御中", "lines": [
            { "description": "x", "quantity": 1, "unit_price_minor": price, "tax_rate_bps": 1000 }
        ]})
    };
    let id = b.create(org, one_line(2)).await;
    // The edited draft keeps its recipient name when the editor does not send one.
    let mut edit = draft();
    edit["lines"] = one_line(2)["lines"].clone();
    app.put(&format!("{}/{id}", b.docs(org)), &b.admin, edit)
        .await;
    let issued = b.issue(org, id).await;
    assert_eq!(issued.body["recipient_name"], "エー製作所 経理部 御中");
    assert_eq!(issued.body["tax_minor"], 1); // 0.2 rounded up
    let credits = format!("{}/{id}/credit-notes", b.docs(org));
    let credit = json!({ "reason": "返品", "lines": [
        { "description": "x", "quantity": 1, "unit_price_minor": 1, "tax_rate_bps": 1000 }
    ]});
    for expected_tax in [1, 0] {
        let cn = uuid(&app.post(&credits, &b.admin, credit.clone()).await.body["id"]);
        let r = b.issue(org, cn).await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
        assert_eq!(
            r.body["tax_minor"], expected_tax,
            "never more tax back than charged"
        );
    }

    // Paid once; paying again is a conflict, not a silent overwrite.
    let pay = format!("{}/{id}/payment", b.docs(org));
    let date = issued.body["issue_date"].clone();
    app.post(&pay, &b.admin, json!({ "paid": true, "paid_on": date }))
        .await;
    let r = app
        .post(&pay, &b.admin, json!({ "paid": true, "paid_on": date }))
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.body["error"]["detail"], "already_paid");

    // Numbers follow dates: no backdating behind the last issued invoice.
    let next = b.create(org, one_line(100)).await;
    let today: chrono::NaiveDate = date.as_str().unwrap().parse().unwrap();
    let r = app
        .post(
            &format!("{}/{next}/issue", b.docs(org)),
            &b.admin,
            json!({ "issue_date": today - chrono::Duration::days(1) }),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(r.body["error"]["detail"], "issue_date");

    // Notes and bank details too long to print in full are refused at issue, not cut off.
    let mut long_bank = issuer();
    long_bank["bank_details"] = json!("振込先の説明です。".repeat(110));
    app.put(&format!("{ADMIN}/billing/issuer"), &b.admin, long_bank)
        .await;
    let mut long = one_line(100);
    long["notes"] = json!("長い備考の文章です。".repeat(200));
    let l = b.create(org, long).await;
    let r = b.issue(org, l).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", r.body);
    assert_eq!(r.body["error"]["detail"], "text_too_long");
}
