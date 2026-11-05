//! Integrations (L7, docs/agents/INTEGRATIONS.md): purchase orders, goods receipts and
//! supplier invoices imported from the factory's files go through preview (no side effects),
//! the `import` agent's cross-check and proposal, and an all-or-nothing apply through the same
//! code as manual entry. Plus duplicates, tenant and permission denial, malicious files and the
//! agent-results export.

// Test-only scenario helpers: a failed unwrap is a failed test, which is the intent.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use axum::http::StatusCode;
use base64::Engine;
use common::*;
use procure_server::agents::worker;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

/// Org A with an assigned operator, a requester, a manager, finance and an approved supplier.
struct Flow {
    w: World,
    operator: String,
    requester: String,
    manager: String,
    finance: String,
    supplier: Uuid,
}

/// A request taken to `approved` (not ordered yet).
struct Approved {
    request: Uuid,
    number: String,
    mpns: Vec<String>,
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

impl Flow {
    async fn new(pool: PgPool) -> Self {
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
        };
        let r =
            f.w.app
                .post(
                    "/api/v1/suppliers",
                    &f.operator,
                    json!({ "name": "Tokyo Bearings KK", "country": "jp",
                            "email": "rfq@tokyo-bearings.test", "categories": ["bearings"] }),
                )
                .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let s = uuid(&r.body["id"]);
        f.ok(f
            .w
            .app
            .post(
                &format!("/api/v1/suppliers/{s}/status"),
                &f.operator,
                json!({ "status": "verified" }),
            )
            .await);
        f.ok(f.op(&format!("suppliers/{s}/proposal"), json!({})).await);
        f.ok(f
            .w
            .app
            .put(
                &f.url(&format!("suppliers/{s}/decision")),
                &f.manager,
                json!({ "decision": "approved" }),
            )
            .await);
        f.supplier = s;
        f
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

    async fn status(&self, request: Uuid) -> String {
        let r = self
            .w
            .app
            .get(&self.url(&format!("requests/{request}")), &self.operator)
            .await;
        r.body["request"]["status"].as_str().unwrap().to_string()
    }

    /// A bearing (100 pcs at 1,200 yen) and a linear guide (4 sets at 45,000 yen), shipping
    /// 2,000 yen, approved by the manager.
    async fn approved(&self) -> Approved {
        let app = &self.w.app;
        let r = app
            .post(
                &self.url("requests"),
                &self.requester,
                json!({
                    "title": "ベアリング交換部品", "currency": "JPY", "needed_by": "2026-12-15",
                    "items": [
                        { "description": "Deep groove ball bearing", "manufacturer": "NSK",
                          "mpn": "6204ZZ", "quantity": 100 },
                        { "description": "Linear guide", "mpn": "HSR15", "quantity": 4, "unit": "set" }
                    ]
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
        let number = d.body["request"]["number"].as_str().unwrap().to_string();
        let items: Vec<Uuid> = d.body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| uuid(&i["id"]))
            .collect();
        self.ok(self
            .op(&format!("requests/{request}/pick-up"), json!({}))
            .await);
        let tag = &request.to_string()[24..];
        let mut mpns = vec![];
        for (i, item) in items.iter().enumerate() {
            let mpn = format!("V{i}-{tag}");
            let r = app
                .post(
                    "/api/v1/products",
                    &self.operator,
                    json!({ "manufacturer": "NSK", "mpn": mpn, "description": "Part",
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
            mpns.push(mpn);
        }
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
                    "lines": [
                        { "request_item_id": items[0], "unit_price_minor": 1200,
                          "quantity": 100, "lead_time_days": 7 },
                        { "request_item_id": items[1], "unit_price_minor": 45_000,
                          "quantity": 4, "lead_time_days": 21 }
                    ]
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
        Approved {
            request,
            number,
            mpns,
        }
    }

    async fn enable_agents(&self) {
        let r = self
            .w
            .app
            .put(
                &self.url("agents/settings"),
                &self.w.admin_a,
                json!({ "enabled": true, "auto_send_supplier_mail": false,
                        "monthly_model_call_budget": 100 }),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    }

    async fn upload(&self, token: &str, kind: &str, name: &str, bytes: &[u8]) -> Resp {
        self.w
            .app
            .post(
                &self.url("integrations/imports"),
                token,
                json!({ "kind": kind, "file_name": name, "data_base64": b64(bytes) }),
            )
            .await
    }

    async fn order(&self, request: Uuid) -> Resp {
        self.w
            .app
            .get(
                &self.url(&format!("requests/{request}/order")),
                &self.manager,
            )
            .await
    }

    /// Runs queued agent work; returns the open exceptions of the import agent.
    async fn run_agents(&self) -> Vec<Value> {
        while worker::run_next(&self.w.app.state).await.unwrap().is_some() {}
        let r = self
            .w
            .app
            .get(
                &self.url("agents/exceptions?agent=import&status=open"),
                &self.w.admin_a,
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
        r.body.as_array().unwrap().clone()
    }

    async fn decide(&self, token: &str, ex: &Value, decision: Value) -> Resp {
        self.w
            .app
            .post(
                &self.url(&format!(
                    "agents/exceptions/{}/decide",
                    ex["id"].as_str().unwrap()
                )),
                token,
                decision,
            )
            .await
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
}

fn csv(rows: &[&[&str]]) -> Vec<u8> {
    let mut s = String::new();
    for r in rows {
        s.push_str(&r.join(","));
        s.push_str("\r\n");
    }
    s.into_bytes()
}

fn sjis(rows: &[&[&str]]) -> Vec<u8> {
    let text = String::from_utf8(csv(rows)).unwrap();
    encoding_rs::SHIFT_JIS.encode(&text).0.into_owned()
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn po_receipt_and_invoice_files_go_through_preview_proposal_and_manual_paths(pool: PgPool) {
    let f = Flow::new(pool).await;
    f.enable_agents().await;
    let a = f.approved().await;

    // 1. Purchase orders (Shift_JIS CSV with Japanese headers and a title row).
    let po_file = sjis(&[
        &["発注データ"],
        &["発注番号", "依頼番号", "発注日", "品番", "数量", "単価"],
        &[
            "PO-2026-001",
            &a.number,
            "2026/10/01",
            "6204ZZ",
            "100",
            "\"1,200\"",
        ],
        &[
            "PO-2026-001",
            &a.number,
            "2026/10/01",
            "HSR15",
            "4",
            "\"45,000\"",
        ],
    ]);
    let r = f.upload(&f.manager, "po", "発注.csv", &po_file).await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["status"], "previewed");
    assert_eq!(r.body["records"], 1);
    assert_eq!(r.body["errors"], json!([]));
    assert_eq!(r.body["staged"][0]["request_number"], a.number);
    let po_batch = uuid(&r.body["id"]);
    // Preview changes nothing in the business tables.
    assert_eq!(f.status(a.request).await, "approved");
    assert_eq!(f.order(a.request).await.status, StatusCode::NOT_FOUND);
    assert!(f.audit("order.recorded").await.is_empty());

    // The agent checks the batch and proposes applying it.
    let open = f.run_agents().await;
    assert_eq!(open.len(), 1, "{open:?}");
    assert_eq!(open[0]["kind"], "import_ready");
    assert_eq!(open[0]["severity"], "info", "{open:?}");
    assert_eq!(open[0]["title_key"], "integrations.import_ready");
    assert_eq!(
        open[0]["proposed_action"],
        json!({ "type": "apply_import", "batch_id": po_batch })
    );
    // Our operator may decide exceptions but not record the factory's orders: refused, and
    // the decision is undone with it.
    let r = f
        .decide(&f.operator, &open[0], json!({ "decision": "approve" }))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{:?}", r.body);
    assert_eq!(f.status(a.request).await, "approved");
    // An edit cannot point the action at another batch.
    let r = f
        .decide(
            &f.manager,
            &open[0],
            json!({ "decision": "edit",
                    "action": { "type": "apply_import", "batch_id": Uuid::now_v7() } }),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", r.body);
    let r = f
        .decide(&f.manager, &open[0], json!({ "decision": "approve" }))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(f.status(a.request).await, "ordered");
    let order = f.ok(f.order(a.request).await);
    assert_eq!(order["order"]["po_number"], "PO-2026-001");
    // The manual path's own events, plus one per applied record.
    assert_eq!(f.audit("order.recorded").await.len(), 1);
    let applied = f.audit("import.record_applied").await;
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0]["data"]["rows"], json!([3, 4]));
    let r =
        f.w.app
            .get(
                &f.url(&format!("integrations/imports/{po_batch}")),
                &f.manager,
            )
            .await;
    assert_eq!(r.body["status"], "applied");

    // 2. Receipts: matched by part number (the verified one), applied directly.
    let r = f
        .upload(
            &f.requester,
            "receipt",
            "受入.csv",
            &csv(&[
                &["発注番号", "納品日", "品番", "数量"],
                &["PO-2026-001", "2026/10/10", &a.mpns[0], "100"],
                &["PO-2026-001", "2026/10/10", "HSR15", "4"],
            ]),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let receipt_batch = uuid(&r.body["id"]);
    // The requester sees only their own requests (this one is theirs) and has no say over
    // exceptions; they apply the batch directly.
    let r =
        f.w.app
            .post(
                &f.url(&format!("integrations/imports/{receipt_batch}/apply")),
                &f.requester,
                json!({}),
            )
            .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["applied"][0]["entity_type"], "delivery");
    assert_eq!(f.status(a.request).await, "delivered");
    // The agent's proposal for that batch was closed as handled elsewhere.
    assert!(f.run_agents().await.is_empty());

    // 3. Invoice with a price difference on line 1: the same three-way match as manual entry.
    let r = f
        .upload(
            &f.finance,
            "invoice",
            "請求.csv",
            &csv(&[
                &[
                    "請求番号",
                    "発注番号",
                    "請求日",
                    "行番号",
                    "数量",
                    "単価",
                    "消費税",
                    "送料",
                ],
                &[
                    "INV-77",
                    "PO-2026-001",
                    "2026/10/20",
                    "1",
                    "100",
                    "1300",
                    "31200",
                    "2000",
                ],
                &[
                    "INV-77",
                    "PO-2026-001",
                    "2026/10/20",
                    "2",
                    "4",
                    "45000",
                    "31200",
                    "2000",
                ],
            ]),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let open = f.run_agents().await;
    assert_eq!(open.len(), 1);
    assert_eq!(open[0]["kind"], "import_ready");
    assert_eq!(open[0]["severity"], "warning");
    let finding = &open[0]["params"]["findings"][0];
    assert_eq!(finding["code"], "amount_mismatch");
    assert_eq!(finding["expected"], 1200);
    assert_eq!(finding["actual"], 1300);
    let r = f
        .decide(&f.finance, &open[0], json!({ "decision": "approve" }))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(f.status(a.request).await, "disputed");
    let order = f.ok(f.order(a.request).await);
    let inv = &order["invoice"];
    assert_eq!(inv["supplier_invoice_number"], "INV-77");
    assert_eq!(inv["match_status"], "disputed");
    let kinds: Vec<&str> = order["discrepancies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["price_mismatch"], "{order:?}");
    let recorded = f.audit("invoice.recorded").await;
    assert_eq!(recorded[0]["data"]["match"], "disputed");
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn a_matching_imported_invoice_reconciles_like_a_manual_one(pool: PgPool) {
    let f = Flow::new(pool).await;
    let a = f.approved().await;
    // Agents off: nothing is proposed, people apply batches directly.
    for (token, kind, rows) in [
        (
            &f.manager,
            "po",
            csv(&[
                &["PO Number", "Request Number"],
                &["PO-9", a.number.as_str()],
            ]),
        ),
        (
            &f.manager,
            "receipt",
            csv(&[
                &["PO Number", "Received Date", "Line", "Qty"],
                &["PO-9", "2026-10-02", "1", "100"],
                &["PO-9", "2026-10-02", "2", "4"],
            ]),
        ),
        (
            &f.finance,
            "invoice",
            csv(&[
                &[
                    "Invoice No",
                    "PO Number",
                    "Invoice Date",
                    "Line",
                    "Qty",
                    "Amount",
                    "Tax",
                    "Shipping",
                ],
                &[
                    "I-1",
                    "PO-9",
                    "2026-10-20",
                    "1",
                    "100",
                    "\"120,000\"",
                    "30200",
                    "2000",
                ],
                &[
                    "I-1",
                    "PO-9",
                    "2026-10-20",
                    "2",
                    "4",
                    "\"180,000\"",
                    "30200",
                    "2000",
                ],
            ]),
        ),
    ] {
        let r = f.upload(token, kind, &format!("{kind}.csv"), &rows).await;
        assert_eq!(r.status, StatusCode::CREATED, "{kind}: {:?}", r.body);
        assert_eq!(r.body["records"], 1, "{kind}: {:?}", r.body);
        let id = uuid(&r.body["id"]);
        let r =
            f.w.app
                .post(
                    &f.url(&format!("integrations/imports/{id}/apply")),
                    token,
                    json!({}),
                )
                .await;
        assert_eq!(r.status, StatusCode::OK, "{kind}: {:?}", r.body);
    }
    assert!(f.run_agents().await.is_empty());
    assert_eq!(f.status(a.request).await, "reconciled");
    let order = f.ok(f.order(a.request).await);
    assert_eq!(order["invoice"]["match_status"], "matched");
    // Reconciled through the same path: the order also went into the purchase history.
    assert_eq!(f.audit("request.reconciled").await.len(), 1);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn apply_is_all_or_nothing_and_files_are_imported_once(pool: PgPool) {
    let f = Flow::new(pool).await;
    f.enable_agents().await;
    let a = f.approved().await;
    let po = csv(&[&["発注番号", "依頼番号"], &["PO-1", a.number.as_str()]]);
    let r = f.upload(&f.manager, "po", "po.csv", &po).await;
    let id = uuid(&r.body["id"]);
    f.ok(f
        .w
        .app
        .post(
            &f.url(&format!("integrations/imports/{id}/apply")),
            &f.manager,
            json!({}),
        )
        .await);
    // Applying again is refused.
    let r =
        f.w.app
            .post(
                &f.url(&format!("integrations/imports/{id}/apply")),
                &f.manager,
                json!({}),
            )
            .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.body["error"]["detail"], "import_not_previewed");
    // The same file again (same SHA-256), under another name: refused.
    let r = f.upload(&f.manager, "po", "again.csv", &po).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{:?}", r.body);
    assert_eq!(r.body["error"]["detail"], "duplicate_file");

    // A receipt file whose second record names an unknown PO: the agent blocks it, and a
    // direct apply writes nothing at all.
    let receipts = csv(&[
        &["発注番号", "納品日", "行番号", "数量"],
        &["PO-1", "2026/10/10", "1", "100"],
        &["PO-404", "2026/10/10", "1", "5"],
    ]);
    let r = f.upload(&f.manager, "receipt", "r.csv", &receipts).await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["records"], 2);
    let rid = uuid(&r.body["id"]);
    let open = f.run_agents().await;
    let blocked: Vec<&Value> = open
        .iter()
        .filter(|e| e["subject_id"] == rid.to_string())
        .collect();
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0]["kind"], "import_blocked");
    assert_eq!(blocked[0]["proposed_action"], Value::Null);
    assert_eq!(blocked[0]["params"]["findings"][0]["code"], "unknown_po");
    assert_eq!(blocked[0]["params"]["findings"][0]["rows"], json!([3]));
    let r =
        f.w.app
            .post(
                &f.url(&format!("integrations/imports/{rid}/apply")),
                &f.manager,
                json!({}),
            )
            .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(r.body["error"]["detail"], "import_unknown_po");
    assert_eq!(f.status(a.request).await, "ordered");
    let order = f.ok(f.order(a.request).await);
    assert_eq!(order["deliveries"], json!([]));
    assert!(f.audit("delivery.recorded").await.is_empty());
    // Discarded, the same file may be uploaded again (e.g. after the PO is entered).
    let r =
        f.w.app
            .post(
                &f.url(&format!("integrations/imports/{rid}/discard")),
                &f.manager,
                json!({}),
            )
            .await;
    assert_eq!(r.body["status"], "failed", "{:?}", r.body);
    assert_eq!(r.body["failure_code"], "discarded");
    assert!(
        f.run_agents()
            .await
            .iter()
            .all(|e| e["subject_id"] != rid.to_string())
    );
    let r = f.upload(&f.manager, "receipt", "r.csv", &receipts).await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);

    // Row errors: the record with a bad row is held back, the rest previewed.
    let r = f
        .upload(
            &f.manager,
            "receipt",
            "bad.csv",
            &csv(&[
                &["発注番号", "納品日", "行番号", "数量"],
                &["PO-1", "2026/13/40", "1", "1"],
                &["PO-1", "2026/10/11", "2", "x"],
            ]),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["status"], "failed");
    assert_eq!(r.body["failure_code"], "nothing_to_import");
    assert_eq!(
        r.body["errors"],
        json!([
            { "row": 2, "field": "received_date", "code": "date_invalid" },
            { "row": 3, "field": "quantity", "code": "quantity_invalid" }
        ])
    );
    assert_eq!(r.body["error_rows"], 2);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn other_tenants_and_roles_without_permission_are_refused(pool: PgPool) {
    let f = Flow::new(pool).await;
    let a = f.approved().await;
    let po = csv(&[&["発注番号", "依頼番号"], &["PO-1", a.number.as_str()]]);
    // Requesters may not record orders; finance may not either.
    for token in [&f.requester, &f.finance] {
        let r = f.upload(token, "po", "po.csv", &po).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{:?}", r.body);
    }
    // Our operator works for the factory but never records its orders.
    let r = f.upload(&f.operator, "po", "po.csv", &po).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{:?}", r.body);
    let r = f.upload(&f.manager, "po", "po.csv", &po).await;
    assert_eq!(r.status, StatusCode::CREATED);
    let id = uuid(&r.body["id"]);
    // Finance sees batches but cannot apply a PO batch.
    let r =
        f.w.app
            .post(
                &f.url(&format!("integrations/imports/{id}/apply")),
                &f.finance,
                json!({}),
            )
            .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // Another organisation: nothing exists.
    let b = |path: &str| format!("/api/v1/orgs/{}/{path}", f.w.org_a);
    for (method, path) in [
        ("get", format!("integrations/imports/{id}")),
        ("get", "integrations/imports".to_string()),
        ("post", format!("integrations/imports/{id}/apply")),
        ("post", format!("integrations/imports/{id}/discard")),
        ("get", "integrations/profiles".to_string()),
        ("get", "integrations/exports/agent-results.csv".to_string()),
    ] {
        let r = match method {
            "get" => f.w.app.get(&b(&path), &f.w.admin_b).await,
            _ => f.w.app.post(&b(&path), &f.w.admin_b, json!({})).await,
        };
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}");
    }
    // Org B's own view of the batch id: not there either (RLS + org filter).
    let r =
        f.w.app
            .get(
                &format!("/api/v1/orgs/{}/integrations/imports/{id}", f.w.org_b),
                &f.w.admin_b,
            )
            .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r =
        f.w.app
            .post(
                &format!("/api/v1/orgs/{}/integrations/imports", f.w.org_b),
                &f.w.admin_b,
                json!({ "kind": "po", "file_name": "po.csv", "data_base64": b64(&po) }),
            )
            .await;
    // Org B may import the same bytes (it is its own file); the request number is unknown
    // there, which apply reports.
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let bid = uuid(&r.body["id"]);
    let r =
        f.w.app
            .post(
                &format!(
                    "/api/v1/orgs/{}/integrations/imports/{bid}/apply",
                    f.w.org_b
                ),
                &f.w.admin_b,
                json!({}),
            )
            .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(r.body["error"]["detail"], "import_unknown_request");
    assert_eq!(f.status(a.request).await, "approved");
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn malicious_files_are_refused_and_exports_never_run_formulas(pool: PgPool) {
    let f = Flow::new(pool).await;
    f.enable_agents().await;
    let m = &f.manager;
    // Wrong type: a PDF or an executable named .csv, a non-zip .xlsx, an unknown extension.
    for (name, bytes) in [
        ("x.csv", b"%PDF-1.7\n".to_vec()),
        ("x.csv", b"MZ\x90\x00\x03".to_vec()),
        ("x.xlsx", b"a,b\n1,2".to_vec()),
        ("x.exe", b"a,b".to_vec()),
        ("x.csv", b"\x89PNG\r\n\x1a\nxxxx".to_vec()),
    ] {
        let r = f.upload(m, "receipt", name, &bytes).await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{name}");
        assert_eq!(r.body["error"]["detail"], "unsupported", "{name}");
    }
    // Empty and oversized files.
    let r = f.upload(m, "receipt", "x.csv", b"").await;
    assert_eq!(r.body["error"]["detail"], "file_size");
    let big = vec![b'a'; 10 * 1024 * 1024 + 1];
    let r = f.upload(m, "receipt", "x.csv", &big).await;
    assert!(
        r.status == StatusCode::UNPROCESSABLE_ENTITY || r.status == StatusCode::PAYLOAD_TOO_LARGE,
        "{}",
        r.status
    );
    // No header with the required columns; bad base64; unknown kind.
    let r = f
        .upload(m, "receipt", "x.csv", "品名,数量\nボルト,1\n".as_bytes())
        .await;
    assert_eq!(r.body["error"]["detail"], "header_not_found");
    let r =
        f.w.app
            .post(
                &f.url("integrations/imports"),
                m,
                json!({ "kind": "receipt", "file_name": "x.csv", "data_base64": "***" }),
            )
            .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = f.upload(m, "payment", "x.csv", b"a").await;
    assert_eq!(r.body["error"]["detail"], "kind");

    // A file of formula-looking cells is data: stored as text, and every CSV cell going back
    // out starting with = + - @ is quoted with '.
    let r = f
        .upload(
            m,
            "receipt",
            "=cmd.csv",
            &csv(&[
                &["発注番号", "納品日", "品番", "数量"],
                &["=HYPERLINK(\"http://evil\")", "2026/10/10", "@SUM(A1)", "1"],
            ]),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(
        r.body["staged"][0]["po_number"],
        "=HYPERLINK(\"http://evil\")"
    );
    let open = f.run_agents().await;
    assert_eq!(open[0]["kind"], "import_blocked");
    let r = f
        .decide(m, &open[0], json!({ "decision": "reject", "note": "-2+3" }))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let (status, ct, body) =
        f.w.app
            .get_raw(
                &f.url("integrations/exports/agent-results.csv?agent=import"),
                m,
            )
            .await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.starts_with("text/csv"), "{ct}");
    let text = String::from_utf8(body).unwrap();
    assert!(text.starts_with("\u{feff}created_at,agent,kind,"), "{text}");
    let row = text.lines().nth(1).unwrap();
    assert!(
        row.contains(",import,import_blocked,critical,rejected,"),
        "{row}"
    );
    assert!(row.ends_with(",'-2+3"), "{row}");
    for line in text.lines() {
        for cell in line.split(',') {
            assert!(
                !cell.starts_with(['=', '+', '-', '@']),
                "unquoted formula cell {cell:?} in {line}"
            );
        }
    }
    let r =
        f.w.app
            .get(
                &f.url("integrations/exports/agent-results.csv?status=maybe"),
                m,
            )
            .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // Requesters do not see agent results.
    let r =
        f.w.app
            .get(
                &f.url("integrations/exports/agent-results.csv"),
                &f.requester,
            )
            .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn saved_profiles_map_an_erps_own_headers(pool: PgPool) {
    let f = Flow::new(pool).await;
    let a = f.approved().await;
    let m = &f.manager;
    let r = f.w.app.get(&f.url("integrations/profiles"), m).await;
    assert_eq!(r.status, StatusCode::OK);
    let names: Vec<&str> = r
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"generic_ja_po") && names.contains(&"generic_en_invoice"));
    // An export with its own header words, day-first dates and European number separators.
    let config = json!({
        "kind": "po",
        "columns": [ { "source": "伝票No", "field": "po_number" },
                     { "source": "社内依頼", "field": "request_number" },
                     { "source": "伝票日付", "field": "order_date" } ],
        "date_format": "dmy", "decimal_separator": ",", "thousands_separator": ".",
        "encoding": "utf-8"
    });
    let r =
        f.w.app
            .put(
                &f.url("integrations/profiles/factory_erp_po"),
                m,
                config.clone(),
            )
            .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["config"]["date_format"], "dmy");
    // Preset names are reserved; invalid settings and names are refused.
    let r =
        f.w.app
            .put(
                &f.url("integrations/profiles/generic_ja_po"),
                m,
                config.clone(),
            )
            .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let mut bad = config.clone();
    bad["thousands_separator"] = json!(",");
    let r = f.w.app.put(&f.url("integrations/profiles/x"), m, bad).await;
    assert_eq!(r.body["error"]["detail"], "profile_separators");
    let r =
        f.w.app
            .put(
                &f.url("integrations/profiles/Bad%20Name"),
                m,
                config.clone(),
            )
            .await;
    assert_eq!(r.body["error"]["detail"], "profile_name");
    // Finance cannot save a PO profile.
    let r =
        f.w.app
            .put(&f.url("integrations/profiles/fin_po"), &f.finance, config)
            .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    let file = csv(&[
        &["伝票No", "社内依頼", "伝票日付", "品番", "数量", "単価"],
        &[
            "PO-77",
            a.number.as_str(),
            "05/10/2026",
            "6204ZZ",
            "100",
            "1.200",
        ],
    ]);
    let r =
        f.w.app
            .post(
                &f.url("integrations/imports"),
                m,
                json!({ "kind": "po", "file_name": "erp.csv", "data_base64": b64(&file),
                    "profile": "factory_erp_po" }),
            )
            .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["profile"], "factory_erp_po");
    assert_eq!(r.body["staged"][0]["date"], "2026-10-05");
    assert_eq!(r.body["staged"][0]["lines"][0]["unit_price_minor"], 1200);
    // A PO profile for an invoice file is refused.
    let r =
        f.w.app
            .post(
                &f.url("integrations/imports"),
                &f.finance,
                json!({ "kind": "invoice", "file_name": "i.csv", "data_base64": b64(&file),
                    "profile": "factory_erp_po" }),
            )
            .await;
    assert_eq!(r.body["error"]["detail"], "profile_kind");
    let r =
        f.w.app
            .delete(&f.url("integrations/profiles/factory_erp_po"), m)
            .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(f.audit("integration.profile_saved").await.len(), 1);
    assert_eq!(f.audit("integration.profile_deleted").await.len(), 1);
}
