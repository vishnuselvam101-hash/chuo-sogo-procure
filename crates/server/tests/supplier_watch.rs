//! Supplier lifecycle (L6): compliance documents kept by our staff (registration number
//! check, customer visibility, internal notes), the `supplier_watch` agent (expiry windows,
//! scorecard review, dedupe, approval into a test-mode draft or a staff task, with audit),
//! scorecards per customer, tenant isolation and permissions.

// Test-only scenario helpers: a failed unwrap is a failed test, which is the intent.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use axum::http::StatusCode;
use base64::Engine;
use common::*;
use procure_domain::agents::RunStatus;
use procure_server::agents::worker;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

/// Org A with an assigned operator, a requester, a manager, finance and one verified
/// supplier (Japan) that A approved.
struct Flow {
    w: World,
    operator: String,
    requester: String,
    manager: String,
    finance: String,
    supplier: Uuid,
    owner: PgPool,
}

struct Ordered {
    order: Uuid,
    items: Vec<Uuid>,
}

impl Flow {
    async fn new(pool: PgPool) -> Self {
        let owner = pool.clone();
        let w = world(pool).await;
        let operator = w
            .member(
                w.platform_org,
                &w.super_admin,
                "op@chuo-sogo.test",
                "procurement_operator",
            )
            .await;
        let op_id = w.user_id(&operator).await;
        let r = w
            .app
            .post(
                &format!("/api/v1/orgs/{}/assignments", w.org_a),
                &w.super_admin,
                json!({ "user_id": op_id }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let requester = w
            .member(w.org_a, &w.admin_a, "req@a.test", "requester")
            .await;
        let manager = w
            .member(w.org_a, &w.admin_a, "m1@a.test", "procurement_manager")
            .await;
        let finance = w.member(w.org_a, &w.admin_a, "fin@a.test", "finance").await;
        let mut f = Flow {
            w,
            operator,
            requester,
            manager,
            finance,
            supplier: Uuid::nil(),
            owner,
        };
        f.supplier = f.approved_supplier("Tokyo Bearings KK", "jp").await;
        f
    }

    /// A verified supplier proposed to and approved by org A.
    async fn approved_supplier(&self, name: &str, country: &str) -> Uuid {
        let r = self
            .w
            .app
            .post(
                "/api/v1/suppliers",
                &self.operator,
                json!({ "name": name, "country": country,
                        "email": format!("{}@supplier.test", &Uuid::now_v7().to_string()[24..]),
                        "notes": "INTERNAL-SUPPLIER-NOTE" }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let s = uuid(&r.body["id"]);
        self.ok(self
            .w
            .app
            .post(
                &format!("/api/v1/suppliers/{s}/status"),
                &self.operator,
                json!({ "status": "verified" }),
            )
            .await);
        self.ok(self.op(&format!("suppliers/{s}/proposal"), json!({})).await);
        self.ok(self
            .w
            .app
            .put(
                &self.url(&format!("suppliers/{s}/decision")),
                &self.manager,
                json!({ "decision": "approved" }),
            )
            .await);
        s
    }

    fn url(&self, path: &str) -> String {
        format!("/api/v1/orgs/{}/{path}", self.w.org_a)
    }

    fn ok(&self, r: Resp) -> Value {
        assert!(r.status.is_success(), "{}: {:?}", r.status, r.body);
        r.body
    }

    async fn op(&self, path: &str, body: Value) -> Resp {
        self.w.app.post(&self.url(path), &self.operator, body).await
    }

    async fn doc(&self, supplier: Uuid, body: Value) -> Resp {
        self.w
            .app
            .post(
                &format!("/api/v1/suppliers/{supplier}/compliance-docs"),
                &self.operator,
                body,
            )
            .await
    }

    async fn doc_id(&self, supplier: Uuid, body: Value) -> Uuid {
        let r = self.doc(supplier, body).await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        uuid(&r.body["id"])
    }

    /// Turns agents on for org A and runs one manual sweep as of `as_of`.
    async fn sweep(&self, as_of: &str) -> Value {
        let r = self
            .w
            .app
            .put(
                &self.url("agents/settings"),
                &self.w.admin_a,
                json!({ "enabled": true, "auto_send_supplier_mail": false,
                        "monthly_model_call_budget": 2000 }),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
        let r = self
            .w
            .app
            .post(
                &self.url("agents/supplier_watch/run"),
                &self.w.admin_a,
                json!({ "input": { "as_of": as_of } }),
            )
            .await;
        assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.body);
        let run = uuid(&r.body["id"]);
        let (id, status) = worker::run_next(&self.w.app.state).await.unwrap().unwrap();
        assert_eq!((id, status), (run, RunStatus::Succeeded));
        let r = self
            .w
            .app
            .get(
                &self.url("agents/runs?agent=supplier_watch"),
                &self.w.admin_a,
            )
            .await;
        r.body
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == run.to_string())
            .unwrap()["output"]
            .clone()
    }

    async fn open_exceptions(&self) -> Vec<Value> {
        let r = self
            .w
            .app
            .get(
                &self.url("agents/exceptions?agent=supplier_watch"),
                &self.w.admin_a,
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
        r.body.as_array().unwrap().clone()
    }

    async fn decide(&self, token: &str, id: &Value, body: Value) -> Resp {
        self.w
            .app
            .post(
                &self.url(&format!(
                    "agents/exceptions/{}/decide",
                    id.as_str().unwrap()
                )),
                token,
                seen(&self.w.app, self.w.org_a, token, id.as_str().unwrap(), body).await,
            )
            .await
    }

    /// Rows of `sql` as JSON, read as the owner in org A's context with the staff scope on.
    async fn rows(&self, sql: &str) -> Vec<Value> {
        let mut tx = self.owner.begin().await.unwrap();
        sqlx::query(
            "SELECT set_config('app.org_id', $1, true), set_config('app.staff', 'on', true)",
        )
        .bind(self.w.org_a.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
        let rows: Vec<Value> = sqlx::query_scalar(&format!("SELECT to_jsonb(t) FROM ({sql}) t"))
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        rows
    }

    async fn as_owner(&self, sql: &str, id: Uuid, value: &str) {
        let mut tx = self.owner.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.org_id', $1, true)")
            .bind(self.w.org_a.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        let n = sqlx::query(sql)
            .bind(id)
            .bind(value)
            .execute(&mut *tx)
            .await
            .unwrap();
        assert_eq!(n.rows_affected(), 1);
        tx.commit().await.unwrap();
    }

    async fn audit(&self, action: &str) -> Vec<Value> {
        let r = self
            .w
            .app
            .get(&self.url("audit?limit=500"), &self.w.admin_a)
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

    /// A request quoted by the supplier, approved and ordered as `po` at `ordered_at`.
    async fn ordered(&self, po: &str, ordered_at: &str) -> Ordered {
        let app = &self.w.app;
        let r = app
            .post(
                &self.url("requests"),
                &self.requester,
                json!({
                    "title": "ベアリング", "currency": "JPY", "needed_by": "2026-12-15",
                    "items": [{ "description": "Deep groove ball bearing", "manufacturer": "NSK",
                                "mpn": "6204ZZ", "quantity": 100 }]
                }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let request = uuid(&r.body["id"]);
        self.ok(app
            .post(
                &self.url(&format!("requests/{request}/submit")),
                &self.requester,
                json!({}),
            )
            .await);
        let d = app
            .get(&self.url(&format!("requests/{request}")), &self.requester)
            .await;
        let item = uuid(&d.body["items"][0]["id"]);
        self.ok(self
            .op(&format!("requests/{request}/pick-up"), json!({}))
            .await);
        let mpn = format!("VERIFIED-{}", &request.to_string()[24..]);
        let r = app
            .post(
                "/api/v1/products",
                &self.operator,
                json!({ "manufacturer": "NSK", "mpn": mpn, "description": "Ball bearing",
                        "specs": {}, "evidence": [{ "kind": "datasheet",
                        "url": "https://example.test/datasheet.pdf" }] }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let product = uuid(&r.body["id"]);
        self.ok(app
            .post(
                &format!("/api/v1/products/{product}/status"),
                &self.operator,
                json!({ "status": "verified" }),
            )
            .await);
        self.ok(self
            .op(
                &format!("requests/{request}/items/{item}/verification"),
                json!({ "product_id": product }),
            )
            .await);
        self.ok(self
            .op(&format!("requests/{request}/start-sourcing"), json!({}))
            .await);
        let r = self
            .op(
                &format!("requests/{request}/rfqs"),
                json!({ "supplier_ids": [self.supplier], "response_due": "2026-11-20",
                        "message": "Please quote." }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let rfq = uuid(&r.body["id"]);
        self.ok(self.op(&format!("rfqs/{rfq}/check"), json!({})).await);
        self.ok(self.op(&format!("rfqs/{rfq}/dispatch"), json!({})).await);
        let r = self
            .op(
                &format!("rfqs/{rfq}/quotations"),
                json!({
                    "supplier_id": self.supplier, "currency": "JPY", "shipping_minor": 2000,
                    "valid_until": "2099-01-01",
                    "lines": [{ "request_item_id": item, "unit_price_minor": 1200,
                                "quantity": 100, "lead_time_days": 7 }]
                }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let quote = uuid(&r.body["id"]);
        let r = self.op(&format!("rfqs/{rfq}/comparisons"), json!({})).await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let cmp = uuid(&r.body["id"]);
        let r = self
            .op(
                &format!("requests/{request}/recommendations"),
                json!({ "comparison_id": cmp, "quotation_id": quote }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let rec = uuid(&r.body["id"]);
        self.ok(self
            .op(&format!("recommendations/{rec}/check"), json!({}))
            .await);
        self.ok(self
            .op(&format!("recommendations/{rec}/send"), json!({}))
            .await);
        let r = app
            .post(
                &self.url(&format!("recommendations/{rec}/decisions")),
                &self.manager,
                json!({ "decision": "approve", "comment": "" }),
            )
            .await;
        assert_eq!(r.body["status"], "approved", "{:?}", r.body);
        let r = app
            .post(
                &self.url(&format!("requests/{request}/order")),
                &self.manager,
                json!({ "po_number": po }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let order = uuid(&r.body["id"]);
        self.as_owner(
            "UPDATE purchase_orders SET ordered_at = $2::timestamptz WHERE id = $1",
            order,
            ordered_at,
        )
        .await;
        let d = app
            .get(
                &self.url(&format!("requests/{request}/order")),
                &self.manager,
            )
            .await;
        let items = d.body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| uuid(&i["id"]))
            .collect();
        Ordered { order, items }
    }

    async fn deliver(&self, o: &Ordered, received_at: &str) {
        let r = self
            .w
            .app
            .post(
                &self.url(&format!("orders/{}/deliveries", o.order)),
                &self.requester,
                json!({ "received_at": received_at,
                        "lines": [{ "order_item_id": o.items[0], "quantity": 100 }] }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    }
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn compliance_documents_are_staff_records_customers_see_only_what_is_shared(pool: PgPool) {
    let f = Flow::new(pool).await;
    let app = &f.w.app;
    let s = f.supplier;

    // The registration number is T + 13 digits with a valid check digit.
    let r = f
        .doc(
            s,
            json!({ "kind": "qualified_invoice_registration", "reference": "T8000012050002" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", r.body);
    assert_eq!(r.body["error"]["detail"], "reference");
    let r = f
        .doc(
            s,
            json!({ "kind": "qualified_invoice_registration", "reference": "7000012050002" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    let r = f.doc(s, json!({ "kind": "iso27001" })).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    let r = f
        .doc(
            s,
            json!({ "kind": "iso9001", "issued_on": "2026-05-01", "expires_on": "2026-04-30" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    let invoice_reg = f
        .doc_id(
            s,
            json!({ "kind": "qualified_invoice_registration", "reference": "t7000-0120-50002",
                    "customer_visible": true, "notes": "INTERNAL-DOC-NOTE" }),
        )
        .await;
    // Shared, with a file.
    let pdf = b"%PDF-1.7 certificate";
    let iso = f
        .doc_id(
            s,
            json!({ "kind": "iso9001", "reference": "JQA-QMA1234", "issued_on": "2024-01-10",
                    "expires_on": "2027-01-09", "customer_visible": true,
                    "notes": "INTERNAL-DOC-NOTE",
                    "file_name": "iso9001.pdf",
                    "data_base64": base64::engine::general_purpose::STANDARD.encode(pdf) }),
        )
        .await;
    // Kept internal.
    let hidden = f
        .doc_id(
            s,
            json!({ "kind": "anti_social_check", "issued_on": "2026-04-01",
                    "expires_on": "2027-03-31", "notes": "Checked via database X" }),
        )
        .await;
    // A file whose content is not what its name says is refused.
    let r = f
        .doc(
            s,
            json!({ "kind": "nda", "file_name": "nda.pdf",
                    "data_base64": base64::engine::general_purpose::STANDARD.encode(b"<html>") }),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(r.body["error"]["detail"], "file_type");

    // Staff see everything, with notes; the number is stored normalised.
    let r = app
        .get(
            &format!("/api/v1/suppliers/{s}/compliance-docs"),
            &f.operator,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let docs = r.body.as_array().unwrap();
    assert_eq!(docs.len(), 3);
    let reg = docs
        .iter()
        .find(|d| d["id"] == invoice_reg.to_string())
        .unwrap();
    assert_eq!(reg["reference"], "T7000012050002");
    assert_eq!(reg["notes"], "INTERNAL-DOC-NOTE");
    let (status, _, bytes) = app
        .get_raw(
            &format!("/api/v1/suppliers/{s}/compliance-docs/{iso}/file"),
            &f.operator,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, pdf);

    // Verification is by a named person and is cleared when the facts change.
    let r = app
        .post(
            &format!("/api/v1/suppliers/{s}/compliance-docs/{iso}/verify"),
            &f.operator,
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(
        r.body["verified_by"],
        f.w.user_id(&f.operator).await.to_string()
    );
    let update = |expires: &str| {
        json!({ "reference": "JQA-QMA1234", "issued_on": "2024-01-10", "expires_on": expires,
                "customer_visible": true, "notes": "INTERNAL-DOC-NOTE" })
    };
    let r = app
        .put(
            &format!("/api/v1/suppliers/{s}/compliance-docs/{iso}"),
            &f.operator,
            update("2027-01-09"),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert!(r.body["verified_at"].is_string());
    let r = app
        .put(
            &format!("/api/v1/suppliers/{s}/compliance-docs/{iso}"),
            &f.operator,
            update("2027-02-09"),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert!(r.body["verified_at"].is_null());

    // Customers: never the staff routes.
    for token in [&f.w.admin_a, &f.manager] {
        let r = app
            .get(&format!("/api/v1/suppliers/{s}/compliance-docs"), token)
            .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
        let r =
            f.w.app
                .post(
                    &format!("/api/v1/suppliers/{s}/compliance-docs"),
                    token,
                    json!({ "kind": "nda" }),
                )
                .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
        let (status, _, _) = app
            .get_raw(
                &format!("/api/v1/suppliers/{s}/compliance-docs/{iso}/file"),
                token,
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    // The customer's view: shared documents only, no internal fields.
    let r = app
        .get(
            &f.url(&format!("suppliers/{s}/compliance-docs")),
            &f.manager,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let shared = r.body.as_array().unwrap();
    assert_eq!(shared.len(), 2, "{shared:?}");
    assert!(shared.iter().all(|d| d["id"] != hidden.to_string()));
    let text = r.body.to_string();
    assert!(!text.contains("INTERNAL"), "{text}");
    for field in ["notes", "verified_by", "created_by", "file_name"] {
        assert!(shared.iter().all(|d| d.get(field).is_none()), "{field}");
    }
    // A requester has no business here; another customer sees nothing of org A.
    let r = app
        .get(
            &f.url(&format!("suppliers/{s}/compliance-docs")),
            &f.requester,
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = app
        .get(
            &f.url(&format!("suppliers/{s}/compliance-docs")),
            &f.w.admin_b,
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = app
        .get(
            &format!("/api/v1/orgs/{}/suppliers/{s}/compliance-docs", f.w.org_b),
            &f.w.admin_b,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body, json!([]));

    // Retiring hides it from customers; retired documents do not change.
    let mut retire = update("2027-02-09");
    retire["retired"] = json!(true);
    let r = app
        .put(
            &format!("/api/v1/suppliers/{s}/compliance-docs/{iso}"),
            &f.operator,
            retire.clone(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let r = app
        .put(
            &format!("/api/v1/suppliers/{s}/compliance-docs/{iso}"),
            &f.operator,
            retire,
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let r = app
        .get(
            &f.url(&format!("suppliers/{s}/compliance-docs")),
            &f.manager,
        )
        .await;
    assert_eq!(r.body.as_array().unwrap().len(), 1);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn expiring_documents_are_proposed_once_and_approved_into_a_test_mode_draft(pool: PgPool) {
    let f = Flow::new(pool).await;
    let s = f.supplier;
    let other = f.approved_supplier("Acme Tools Ltd", "us").await;
    // As of 2026-11-05 (Japan).
    let soon = f
        .doc_id(
            s,
            json!({ "kind": "iso9001", "expires_on": "2026-12-05", "customer_visible": true }),
        )
        .await; // 30 days
    let urgent = f
        .doc_id(
            other,
            json!({ "kind": "insurance", "expires_on": "2026-11-12", "customer_visible": true }),
        )
        .await; // 7 days
    let expired = f
        .doc_id(
            s,
            json!({ "kind": "nda", "expires_on": "2026-11-04", "customer_visible": true }),
        )
        .await;
    // Not due, internal, or replaced by a newer certificate: nothing.
    f.doc_id(
        s,
        json!({ "kind": "iso14001", "expires_on": "2026-12-31", "customer_visible": true }),
    )
    .await; // 56 days
    f.doc_id(
        s,
        json!({ "kind": "anti_social_check", "expires_on": "2026-11-01" }),
    )
    .await;
    f.doc_id(
        other,
        json!({ "kind": "iatf16949", "expires_on": "2026-11-10", "customer_visible": true }),
    )
    .await;
    f.doc_id(
        other,
        json!({ "kind": "iatf16949", "expires_on": "2029-11-10", "customer_visible": true }),
    )
    .await;
    // A supplier org A did not approve: not A's concern.
    let r =
        f.w.app
            .post(
                "/api/v1/suppliers",
                &f.operator,
                json!({ "name": "Stranger KK", "country": "JP" }),
            )
            .await;
    let stranger = uuid(&r.body["id"]);
    f.doc_id(
        stranger,
        json!({ "kind": "nda", "expires_on": "2026-11-01", "customer_visible": true }),
    )
    .await;

    let out = f.sweep("2026-11-05").await;
    assert_eq!(out["proposed"], 3, "{out:?}");
    let list = f.open_exceptions().await;
    assert_eq!(list.len(), 3, "{list:?}");
    let by_doc = |id: Uuid| {
        list.iter()
            .find(|e| e["subject_id"] == id.to_string())
            .unwrap()
            .clone()
    };
    let e = by_doc(soon);
    assert_eq!(e["kind"], "document_expiring");
    assert_eq!(e["severity"], "info");
    assert_eq!(e["title_key"], "supplier.documentExpiring");
    assert_eq!(e["params"]["window"], "within_30");
    assert_eq!(e["params"]["supplier_name"], "Tokyo Bearings KK");
    assert_eq!(
        e["proposed_action"],
        json!({ "type": "request_document_update", "doc_id": soon })
    );
    let e = by_doc(urgent);
    assert_eq!(e["severity"], "warning");
    assert_eq!(e["params"]["window"], "within_7");
    let e = by_doc(expired);
    assert_eq!(e["kind"], "document_expired");
    assert_eq!(e["severity"], "critical");
    assert_eq!(e["title_key"], "supplier.documentExpired");
    assert!(!list.iter().any(|e| e.to_string().contains("INTERNAL")));

    // The same day again: no duplicates, nothing re-audited.
    f.sweep("2026-11-05").await;
    assert_eq!(f.open_exceptions().await.len(), 3);
    assert_eq!(f.audit("agent.exception_raised").await.len(), 3);

    // Finance may decide exceptions but does not work with suppliers: refused, still open.
    let r = f
        .decide(
            &f.finance,
            &by_doc(soon)["id"],
            json!({ "decision": "approve" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{:?}", r.body);
    // An edit cannot point the request at another document.
    let r = f
        .decide(
            &f.manager,
            &by_doc(soon)["id"],
            json!({ "decision": "edit",
                    "action": { "type": "request_document_update", "doc_id": expired } }),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", r.body);
    assert_eq!(f.open_exceptions().await.len(), 3);

    // Approved: a Japanese draft to the Japanese supplier, recorded in test mode only.
    let r = f
        .decide(
            &f.manager,
            &by_doc(soon)["id"],
            json!({ "decision": "approve" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["status"], "approved");
    // Edited: English for the American supplier, with a note.
    let r = f
        .decide(
            &f.w.admin_a,
            &by_doc(urgent)["id"],
            json!({ "decision": "edit", "note": "",
                    "action": { "type": "request_document_update", "doc_id": urgent,
                                "lang": "en", "note": "A scan is fine." } }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let mail = f
        .rows(
            "SELECT compliance_doc_id, supplier_id, channel, status, kind, subject, body,
                    rfq_recipient_id
             FROM outbox WHERE kind = 'document_request' ORDER BY created_at",
        )
        .await;
    assert_eq!(mail.len(), 2, "{mail:?}");
    assert_eq!(mail[0]["compliance_doc_id"], soon.to_string());
    assert_eq!(mail[0]["supplier_id"], s.to_string());
    assert_eq!(mail[0]["channel"], "test");
    assert_eq!(mail[0]["status"], "recorded");
    assert!(mail[0]["rfq_recipient_id"].is_null());
    assert!(
        mail[0]["subject"]
            .as_str()
            .unwrap()
            .contains("ご提出のお願い"),
        "{mail:?}"
    );
    assert!(
        mail[0]["body"]
            .as_str()
            .unwrap()
            .starts_with("Tokyo Bearings KK 御中")
    );
    assert_eq!(
        mail[1]["subject"],
        "Request for an updated insurance certificate"
    );
    assert!(
        mail[1]["body"]
            .as_str()
            .unwrap()
            .contains("A scan is fine.")
    );
    let audited = f.audit("supplier_watch.document_update_requested").await;
    assert_eq!(audited.len(), 2);
    assert_eq!(f.audit("agent.exception_decided").await.len(), 2);

    // Decided findings are not proposed again; the expired one is still open.
    f.sweep("2026-11-06").await;
    let list = f.open_exceptions().await;
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!(list[0]["subject_id"], expired.to_string());

    // The next window is a new finding; a renewal closes the old one.
    f.sweep("2026-11-28").await; // ISO 9001 now within 7 days
    let list = f.open_exceptions().await;
    let iso = list
        .iter()
        .find(|e| e["subject_id"] == soon.to_string())
        .unwrap();
    assert_eq!(iso["params"]["window"], "within_7");
    let r =
        f.w.app
            .put(
                &format!("/api/v1/suppliers/{s}/compliance-docs/{expired}"),
                &f.operator,
                json!({ "expires_on": "2028-11-04", "customer_visible": true }),
            )
            .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    f.sweep("2026-11-28").await;
    let list = f.open_exceptions().await;
    assert!(
        list.iter().all(|e| e["subject_id"] != expired.to_string()),
        "{list:?}"
    );
    let all =
        f.w.app
            .get(
                &f.url("agents/exceptions?agent=supplier_watch&status=resolved_elsewhere"),
                &f.w.admin_a,
            )
            .await;
    assert_eq!(all.body.as_array().unwrap().len(), 1, "{:?}", all.body);

    // Org B (agents on, no relation to these suppliers) gets nothing and sees nothing of A.
    let r =
        f.w.app
            .get(
                &format!("/api/v1/orgs/{}/agents/exceptions", f.w.org_a),
                &f.w.admin_b,
            )
            .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn scorecards_flag_late_suppliers_for_review_without_touching_the_approval(pool: PgPool) {
    let f = Flow::new(pool).await;
    let s = f.supplier;
    // Three orders promised for 10 October 2026.
    let mut orders = vec![];
    for n in 0..3 {
        let o = f
            .ordered(&format!("PO-SW-{n}"), "2026-10-01T00:00:00Z")
            .await;
        // The order already carries a promised date; the chasing lane's trigger keeps it
        // write-once, so the test fixture lifts the trigger for its own setup only.
        let mut tx = f.owner.begin().await.unwrap();
        sqlx::query("ALTER TABLE purchase_orders DISABLE TRIGGER purchase_orders_promised_once")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('app.org_id', $1, true)")
            .bind(f.w.org_a.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        let n = sqlx::query("UPDATE purchase_orders SET promised_on = '2026-10-10' WHERE id = $1")
            .bind(o.order)
            .execute(&mut *tx)
            .await
            .unwrap();
        assert_eq!(n.rows_affected(), 1);
        sqlx::query("ALTER TABLE purchase_orders ENABLE TRIGGER purchase_orders_promised_once")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        orders.push(o);
    }
    // 23:30 on the 10th in Tokyo is on time; 00:30 on the 11th in Tokyo is late although it is
    // still the 10th in UTC; the third has not arrived (overdue).
    f.deliver(&orders[0], "2026-10-10T23:30:00+09:00").await;
    f.deliver(&orders[1], "2026-10-11T00:30:00+09:00").await;
    // One October invoice with a price difference.
    let r =
        f.w.app
            .post(
                &f.url(&format!("orders/{}/invoice", orders[0].order)),
                &f.finance,
                json!({
                    "supplier_invoice_number": "INV-SW-1", "invoice_date": "2026-10-20",
                    "currency": "JPY", "shipping_minor": 2000, "tax_rate_bps": 1000,
                    "tax_minor": 13_200,
                    "lines": [{ "order_item_id": orders[0].items[0], "quantity": 100,
                                "unit_price_minor": 1300 }]
                }),
            )
            .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["match_status"], "disputed");

    let r =
        f.w.app
            .get(&f.url("supplier-scorecards?month=2026-10"), &f.manager)
            .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let cards = r.body.as_array().unwrap();
    assert_eq!(cards.len(), 1, "{cards:?}");
    let c = &cards[0];
    assert_eq!(c["supplier_id"], s.to_string());
    assert_eq!(c["month"], "2026-10");
    assert_eq!(c["deliveries_measured"], 3);
    assert_eq!(c["on_time_bps"], 3333);
    assert_eq!(c["invoices"], 1);
    assert_eq!(c["discrepancy_bps"], 10_000);
    // Finance (views agents) may read them too.
    let r =
        f.w.app
            .get(&f.url("supplier-scorecards?month=2026-10"), &f.finance)
            .await;
    assert_eq!(r.status, StatusCode::OK);
    // The RFQs went out this month (real clock): every one answered, sole bidder unranked.
    let r = f.w.app.get(&f.url("supplier-scorecards"), &f.manager).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let now = &r.body.as_array().unwrap()[0];
    assert_eq!(now["rfqs_received"], 3);
    assert_eq!(now["reply_rate_bps"], 10_000);
    assert!(now["median_reply_hours"].is_number());
    assert!(now["price_score_bps"].is_null());
    assert_eq!(now["comparisons_ranked"], 0);
    let r =
        f.w.app
            .get(&f.url("supplier-scorecards?month=2026-13"), &f.manager)
            .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);

    // Permissions and tenants.
    let r =
        f.w.app
            .get(&f.url("supplier-scorecards?month=2026-10"), &f.requester)
            .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r =
        f.w.app
            .get(&f.url("supplier-scorecards?month=2026-10"), &f.w.admin_b)
            .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r =
        f.w.app
            .get(
                &format!(
                    "/api/v1/orgs/{}/supplier-scorecards?month=2026-10",
                    f.w.org_b
                ),
                &f.w.admin_b,
            )
            .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body, json!([]));

    // The November sweep looks at October.
    let out = f.sweep("2026-11-05").await;
    assert_eq!(out["scorecard_month"], "2026-10");
    let list = f.open_exceptions().await;
    assert_eq!(list.len(), 1, "{list:?}");
    let e = &list[0];
    assert_eq!(e["kind"], "scorecard_below_threshold");
    assert_eq!(e["title_key"], "supplier.scorecardReview");
    assert_eq!(e["subject_id"], s.to_string());
    // One invoice is too few to judge quality.
    assert_eq!(e["params"]["reasons"], json!(["low_on_time"]));
    assert_eq!(e["proposed_action"]["type"], "flag_supplier_review");
    assert_eq!(e["proposed_action"]["month"], "2026-10");

    // Finance cannot act on it; the manager approves: a task for our staff.
    let r = f
        .decide(&f.finance, &e["id"], json!({ "decision": "approve" }))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = f
        .decide(&f.manager, &e["id"], json!({ "decision": "approve" }))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let tasks = f
        .rows("SELECT kind, title, status, created_by FROM tasks WHERE kind = 'exception'")
        .await;
    assert_eq!(tasks.len(), 1, "{tasks:?}");
    assert!(
        tasks[0]["title"]
            .as_str()
            .unwrap()
            .starts_with("Review supplier Tokyo Bearings KK (2026-10)")
    );
    assert!(tasks[0]["created_by"].is_null());
    // Customers never see staff tasks.
    let r = f.w.app.get("/api/v1/ops/queue", &f.manager).await;
    assert_ne!(r.status, StatusCode::OK);
    // The customer's approval of the supplier is unchanged: people decide that.
    let approval = f
        .rows("SELECT status FROM customer_supplier_approvals")
        .await;
    assert_eq!(approval, vec![json!({ "status": "approved" })]);
    assert_eq!(f.audit("supplier_watch.review_flagged").await.len(), 1);
    assert_eq!(f.audit("supplier.customer_decision").await.len(), 1);

    // Not proposed again for the same month.
    f.sweep("2026-11-06").await;
    assert!(f.open_exceptions().await.is_empty());
}

/// L9 C5: scorecard days are the customer's own calendar days (`organisations.timezone`),
/// not Japan's for everyone.
#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn scorecards_use_the_organisations_time_zone(pool: PgPool) {
    let f = Flow::new(pool).await;
    let mut tx = f.owner.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.scope', 'platform', true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE organisations SET timezone = 'America/Los_Angeles' WHERE id = $1")
        .bind(f.w.org_a)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let o = f.ordered("PO-TZ", "2026-10-01T00:00:00Z").await;
    let mut tx = f.owner.begin().await.unwrap();
    sqlx::query("ALTER TABLE purchase_orders DISABLE TRIGGER purchase_orders_promised_once")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.org_id', $1, true)")
        .bind(f.w.org_a.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE purchase_orders SET promised_on = '2026-10-10' WHERE id = $1")
        .bind(o.order)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE purchase_orders ENABLE TRIGGER purchase_orders_promised_once")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    // 13:00 on the 10th in Los Angeles (already the 11th in Tokyo): on time for this customer.
    f.deliver(&o, "2026-10-10T20:00:00Z").await;
    let r =
        f.w.app
            .get(&f.url("supplier-scorecards?month=2026-10"), &f.manager)
            .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let cards = r.body.as_array().unwrap();
    assert_eq!(cards.len(), 1, "{cards:?}");
    assert_eq!(cards[0]["deliveries_measured"], 1);
    assert_eq!(cards[0]["on_time_bps"], 10_000, "{cards:?}");
}
