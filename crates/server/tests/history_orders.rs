//! Orders completed in this system are recorded into the customer's purchase history when they
//! reconcile (matched invoice, or the last discrepancy resolved), against a real database.

// Test-only scenario helpers: a failed unwrap is a failed test, which is the intent.
#![allow(clippy::unwrap_used)]

mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};
use sha2::Digest;
use sqlx::PgPool;
use uuid::Uuid;

/// Org A (Asia/Tokyo) with an assigned operator, a requester, a manager, finance and one
/// verified supplier A approved.
struct Flow {
    w: World,
    operator: String,
    requester: String,
    manager: String,
    finance: String,
    supplier: Uuid,
    /// The migration owner, for setting up states the API cannot produce.
    owner: PgPool,
}

/// An order taken to `ordered`, with its order item ids and verified part numbers.
struct Ordered {
    request: Uuid,
    rfq: Uuid,
    order: Uuid,
    items: Vec<Uuid>,
    mpns: Vec<String>,
}

impl Flow {
    async fn new(pool: PgPool) -> Self {
        let owner = pool.clone();
        Self::with_world(owner, world(pool).await).await
    }

    async fn with_world(owner: PgPool, w: World) -> Self {
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

    /// Request for a bearing (100 pcs) and a linear guide (4 sets), both verified, quoted by the
    /// supplier at 1,200 and 45,000 yen (line 2 optionally under the supplier's own part number), approved and
    /// ordered as `po`, then backdated to `ordered_at`.
    /// The request of `ordered` up to its RFQ being checked and dispatched:
    /// (request, rfq, request items, verified part numbers).
    async fn sent_rfq(&self) -> (Uuid, Uuid, Vec<Uuid>, Vec<String>) {
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
        let req_items: Vec<Uuid> = d.body["items"]
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
        for (i, item) in req_items.iter().enumerate() {
            let mpn = format!("VERIFIED-{i}-{tag}");
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
        (request, rfq, req_items, mpns)
    }

    async fn ordered(&self, po: &str, offered_mpn: bool, ordered_at: &str) -> Ordered {
        let app = &self.w.app;
        let (request, rfq, req_items, mpns) = self.sent_rfq().await;
        let mut second = json!({ "request_item_id": req_items[1], "unit_price_minor": 45_000,
                                 "quantity": 4, "lead_time_days": 21 });
        if offered_mpn {
            second["offered_mpn"] = json!("HSR15A-ALT");
        }
        let r = self
            .op(
                &format!("rfqs/{rfq}/quotations"),
                json!({
                    "supplier_id": self.supplier, "currency": "JPY", "shipping_minor": 2000,
                    "valid_until": "2099-01-01",
                    "lines": [
                        { "request_item_id": req_items[0], "unit_price_minor": 1200,
                          "quantity": 100, "lead_time_days": 7 },
                        second
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
        let r = app
            .post(
                &self.url(&format!("requests/{request}/order")),
                &self.manager,
                json!({ "po_number": po }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let order = uuid(&r.body["id"]);
        // The API stamps orders with now(); move it back so lead times are measurable.
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
        Ordered {
            request,
            rfq,
            order,
            items,
            mpns,
        }
    }

    /// Runs `sql` (binding $1 = id, $2 = value) as the owner within org A; one row must change.
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

    async fn deliver(&self, o: &Ordered, received_at: &str, lines: Value) -> Uuid {
        let r = self
            .w
            .app
            .post(
                &self.url(&format!("orders/{}/deliveries", o.order)),
                &self.requester,
                json!({ "received_at": received_at, "lines": lines }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        uuid(&r.body["id"])
    }

    async fn invoice(&self, o: &Ordered, first_price: i64, tax: i64) -> Resp {
        self.w
            .app
            .post(
                &self.url(&format!("orders/{}/invoice", o.order)),
                &self.finance,
                json!({
                    "supplier_invoice_number": format!("INV-{}", &o.order.to_string()[24..]),
                    "invoice_date": "2026-10-20", "currency": "JPY", "shipping_minor": 2000,
                    "tax_rate_bps": 1000, "tax_minor": tax,
                    "lines": [
                        { "order_item_id": o.items[0], "quantity": 100, "unit_price_minor": first_price },
                        { "order_item_id": o.items[1], "quantity": 4, "unit_price_minor": 45_000 }
                    ]
                }),
            )
            .await
    }

    async fn imports(&self, token: &str) -> Vec<Value> {
        let r = self
            .w
            .app
            .get(&self.url("purchase-history/imports"), token)
            .await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
        r.body.as_array().unwrap().clone()
    }

    async fn history(&self, q: &str) -> Vec<Value> {
        let r = self
            .w
            .app
            .get(
                &self.url(&format!("purchase-history?q={q}")),
                &self.w.admin_a,
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
        r.body.as_array().unwrap().clone()
    }

    async fn audit_actions(&self) -> (Vec<Value>, bool) {
        let r = self
            .w
            .app
            .get(&self.url("audit?limit=500"), &self.w.admin_a)
            .await;
        (
            r.body["events"].as_array().unwrap().clone(),
            r.body["chain_valid"].as_bool().unwrap(),
        )
    }
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn matched_invoice_records_the_order_into_history(pool: PgPool) {
    let f = Flow::new(pool).await;
    let app = &f.w.app;
    // 2026-10-01 15:30 UTC is 2 October in Tokyo.
    let o = f
        .ordered("PO-2026-0042", false, "2026-10-01T15:30:00Z")
        .await;
    // The bearing arrives in two parts; the last part on 15 October (Tokyo). The guide's first
    // receipt is a mistake that is reversed; it really arrives on 9 October (Tokyo).
    f.deliver(
        &o,
        "2026-10-05T00:00:00Z",
        json!([{ "order_item_id": o.items[0], "quantity": 60 }]),
    )
    .await;
    let wrong = f
        .deliver(
            &o,
            "2026-10-25T00:00:00Z",
            json!([{ "order_item_id": o.items[1], "quantity": 4 }]),
        )
        .await;
    f.ok(app
        .post(
            &f.url(&format!("deliveries/{wrong}/reverse")),
            &f.requester,
            json!({ "reason": "Wrong order" }),
        )
        .await);
    f.deliver(
        &o,
        "2026-10-09T01:00:00Z",
        json!([{ "order_item_id": o.items[1], "quantity": 4 }]),
    )
    .await;
    f.deliver(
        &o,
        "2026-10-14T16:00:00Z",
        json!([{ "order_item_id": o.items[0], "quantity": 40 }]),
    )
    .await;
    assert_eq!(f.status(o.request).await, "delivered");
    // Nothing is recorded before the order completes.
    assert!(f.imports(&f.w.admin_a).await.is_empty());

    let r = f.invoice(&o, 1200, 30_200).await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["match_status"], "matched");
    assert_eq!(f.status(o.request).await, "reconciled");

    // One import from the order, readable by finance too.
    let imports = f.imports(&f.finance).await;
    assert_eq!(imports.len(), 1, "{imports:?}");
    let import = &imports[0];
    assert_eq!(import["source"], "order");
    assert_eq!(uuid(&import["order_id"]), o.order);
    assert_eq!(import["file_name"], "PO PO-2026-0042");
    assert_eq!(import["rows_imported"], 2);
    assert_eq!(import["rows_skipped"], 0);
    assert!(import["imported_by_name"].is_string());
    assert!(import["withdrawn_at"].is_null());
    let import_id = uuid(&import["id"]);

    let rows = f.history("").await;
    assert_eq!(rows.len(), 2, "{rows:?}");
    let bearing = rows.iter().find(|r| r["mpn"] == o.mpns[0]).unwrap();
    assert_eq!(uuid(&bearing["import_id"]), import_id);
    assert_eq!(bearing["purchased_on"], "2026-10-02");
    assert_eq!(bearing["supplier_name"], "Tokyo Bearings KK");
    assert_eq!(bearing["description"], "Deep groove ball bearing");
    assert_eq!(bearing["manufacturer"], "NSK");
    assert_eq!(bearing["quantity_milli"], 100_000);
    assert_eq!(bearing["unit"], "pcs");
    assert_eq!(bearing["unit_price_milli"], 1_200_000);
    assert_eq!(bearing["price_derived"], false);
    assert_eq!(bearing["currency"], "JPY");
    // 2 → 15 October.
    assert_eq!(bearing["lead_time_days"], 13);
    assert_eq!(bearing["notes"], "");
    let guide = rows.iter().find(|r| r["mpn"] == o.mpns[1]).unwrap();
    assert_eq!(guide["unit"], "set");
    assert_eq!(guide["quantity_milli"], 4_000);
    assert_eq!(guide["unit_price_milli"], 45_000_000);
    // The reversed receipt does not count: 2 → 9 October.
    assert_eq!(guide["lead_time_days"], 7);

    // A new request for the same part shows the completed order as its price reference.
    let r = app
        .post(
            &f.url("requests"),
            &f.requester,
            json!({ "title": "軸受補充", "currency": "JPY",
                    "items": [{ "description": "Bearing", "mpn": o.mpns[0].to_lowercase(),
                                "quantity": 20 }] }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let next = uuid(&r.body["id"]);
    let r = app
        .get(
            &f.url(&format!("requests/{next}/price-reference")),
            &f.w.admin_a,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body[0]["matched_on"], "mpn");
    assert_eq!(r.body[0]["summary"]["purchases"], 1);
    assert_eq!(
        r.body[0]["summary"]["last"]["supplier_name"],
        "Tokyo Bearings KK"
    );
    assert_eq!(r.body[0]["summary"]["last"]["unit_price_milli"], 1_200_000);
    assert_eq!(r.body[0]["summary"]["last"]["purchased_on"], "2026-10-02");

    // Audited in the reconciling transaction, and the chain holds.
    let (events, chain_valid) = f.audit_actions().await;
    assert!(chain_valid);
    let recorded: Vec<&Value> = events
        .iter()
        .filter(|e| e["action"] == "purchase_history.recorded_from_order")
        .collect();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0]["entity_type"], "history_import");
    assert_eq!(recorded[0]["entity_id"], import_id.to_string());

    // Closing the request does not record it again.
    let r = app
        .post(
            &f.url(&format!("requests/{}/close", o.request)),
            &f.finance,
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(f.imports(&f.w.admin_a).await.len(), 1);

    // Withdrawn like any import: rows go, the import stays listed, the reference empties.
    let withdraw = f.url(&format!("purchase-history/imports/{import_id}/withdraw"));
    let r = app.post(&withdraw, &f.w.admin_a, json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert!(r.body["withdrawn_at"].is_string());
    assert_eq!(r.body["source"], "order");
    assert!(f.history("").await.is_empty());
    let r = app
        .get(
            &f.url(&format!("requests/{next}/price-reference")),
            &f.w.admin_a,
        )
        .await;
    assert_eq!(r.body[0]["summary"], Value::Null);
    let imports = f.imports(&f.w.admin_a).await;
    assert_eq!(imports.len(), 1);
    assert!(imports[0]["withdrawn_at"].is_string());

    // Tenant isolation: another organisation sees none of it and cannot withdraw it.
    let b = |p: &str| format!("/api/v1/orgs/{}/{p}", f.w.org_b);
    let r = app.get(&b("purchase-history/imports"), &f.w.admin_b).await;
    assert_eq!(r.body, json!([]));
    let r = app.get(&b("purchase-history"), &f.w.admin_b).await;
    assert_eq!(r.body, json!([]));
    let r = app
        .post(
            &b(&format!("purchase-history/imports/{import_id}/withdraw")),
            &f.w.admin_b,
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // Org A's own token cannot reach it through org B either.
    let r = app.get(&b("purchase-history/imports"), &f.w.admin_a).await;
    assert_ne!(r.status, StatusCode::OK);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn disputed_order_is_recorded_only_when_the_last_discrepancy_is_resolved(pool: PgPool) {
    let f = Flow::new(pool).await;
    let app = &f.w.app;
    // Line 2 is quoted under the supplier's own part number.
    let o = f.ordered("PO-7", true, "2026-10-01T00:00:00Z").await;
    // An alternative product (the API only orders compliant quotations, so set it directly):
    // recorded without the requested maker.
    f.as_owner(
        "UPDATE order_items SET alternative = ($2 = 'true') WHERE id = $1",
        o.items[1],
        "true",
    )
    .await;
    // Received the same Tokyo day it was ordered.
    f.deliver(
        &o,
        "2026-10-01T02:00:00Z",
        json!([{ "order_item_id": o.items[0], "quantity": 100 },
                { "order_item_id": o.items[1], "quantity": 4 }]),
    )
    .await;
    // Billed 1,250 instead of 1,200, and the wrong tax.
    let r = f.invoice(&o, 1250, 1).await;
    assert_eq!(r.body["match_status"], "disputed", "{:?}", r.body);
    assert_eq!(r.body["discrepancies"], 2);
    assert!(f.imports(&f.w.admin_a).await.is_empty());

    let d = app
        .get(&f.url(&format!("requests/{}/order", o.request)), &f.finance)
        .await;
    let ids: Vec<String> = d.body["discrepancies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids.len(), 2);
    let resolve = |id: &str| f.url(&format!("discrepancies/{id}/resolve"));
    let r = app
        .post(
            &resolve(&ids[0]),
            &f.finance,
            json!({ "resolution": "credit_note", "note": "CN-12" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(f.status(o.request).await, "disputed");
    assert!(f.imports(&f.w.admin_a).await.is_empty());
    assert!(f.history("").await.is_empty());

    let r = app
        .post(
            &resolve(&ids[1]),
            &f.finance,
            json!({ "resolution": "supplier_corrected", "note": "INV revised" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(f.status(o.request).await, "reconciled");
    let imports = f.imports(&f.w.admin_a).await;
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0]["source"], "order");
    assert_eq!(uuid(&imports[0]["order_id"]), o.order);
    assert_eq!(imports[0]["file_name"], "PO PO-7");

    let rows = f.history("").await;
    assert_eq!(rows.len(), 2, "{rows:?}");
    // The PO price is the agreed price, not the disputed invoice price.
    let bearing = rows.iter().find(|r| r["mpn"] == o.mpns[0]).unwrap();
    assert_eq!(bearing["unit_price_milli"], 1_200_000);
    assert_eq!(bearing["purchased_on"], "2026-10-01");
    assert_eq!(bearing["lead_time_days"], 0);
    // Recorded under the part number actually bought, without the requested maker.
    let alt = rows.iter().find(|r| r["mpn"] == "HSR15A-ALT").unwrap();
    assert_eq!(alt["manufacturer"], "");
    assert_eq!(alt["description"], "Linear guide");

    let (events, chain_valid) = f.audit_actions().await;
    assert!(chain_valid);
    assert_eq!(
        events
            .iter()
            .filter(|e| e["action"] == "purchase_history.recorded_from_order")
            .count(),
        1
    );
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn the_database_keeps_one_record_per_order_and_order_imports_consistent(pool: PgPool) {
    let f = Flow::new(pool).await;
    let o = f.ordered("PO-9", false, "2026-10-01T00:00:00Z").await;
    f.deliver(
        &o,
        "2026-10-02T00:00:00Z",
        json!([{ "order_item_id": o.items[0], "quantity": 100 },
                { "order_item_id": o.items[1], "quantity": 4 }]),
    )
    .await;
    assert_eq!(
        f.invoice(&o, 1200, 30_200).await.body["match_status"],
        "matched"
    );
    let user = f.w.user_id(&f.finance).await;
    let mut tx = f.owner.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE procure_runtime")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.org_id', $1, true)")
        .bind(f.w.org_a.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let insert = |source: &'static str, order: Option<Uuid>| {
        sqlx::query(
            "INSERT INTO history_imports (id, org_id, file_name, mapping, rows_imported,
                                          rows_skipped, imported_by, source, order_id)
             VALUES ($1, $2, 'x', '[]', 0, 0, $3, $4, $5)",
        )
        .bind(Uuid::now_v7())
        .bind(f.w.org_a)
        .bind(user)
        .bind(source)
        .bind(order)
    };
    sqlx::query("SAVEPOINT s").execute(&mut *tx).await.unwrap();
    // A second record of the same order is refused.
    let err = insert("order", Some(o.order))
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unique"), "{err}");
    sqlx::query("ROLLBACK TO SAVEPOINT s")
        .execute(&mut *tx)
        .await
        .unwrap();
    // A file import cannot claim an order, nor an order import go without one.
    for (source, order) in [("file", Some(o.order)), ("order", None)] {
        let err = insert(source, order).execute(&mut *tx).await.unwrap_err();
        assert!(err.to_string().contains("check"), "{err}");
        sqlx::query("ROLLBACK TO SAVEPOINT s")
            .execute(&mut *tx)
            .await
            .unwrap();
    }
}

// ---------------------------------------------------------------------------------------
// Savings share: how Chuo-Sogo is paid (half of the drop from the price paid before).

const ADMIN: &str = "/api/admin/v1";

async fn admin_session(app: &App) -> String {
    let call = |path: &str, body: Value| {
        let uri = format!("{ADMIN}{path}");
        async move {
            app.call(axum::http::Method::POST, &uri, None, Some(body))
                .await
        }
    };
    let r = call(
        "/auth/login",
        json!({ "email": "root@chuo-sogo.test", "password": PASSWORD }),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let token = r.body["token"].as_str().unwrap().to_string();
    let e = call("/auth/totp/enroll", json!({ "token": token })).await;
    let secret = procure_server::mfa::base32_decode(e.body["secret"].as_str().unwrap()).unwrap();
    let step = chrono::Utc::now()
        .timestamp()
        .div_euclid(procure_server::mfa::STEP_SECONDS);
    let code = format!("{:06}", procure_server::mfa::code_at(&secret, step));
    let c = call(
        "/auth/totp/confirm",
        json!({ "token": token, "code": code }),
    )
    .await;
    assert_eq!(c.status, StatusCode::OK, "{:?}", c.body);
    c.body["token"].as_str().unwrap().to_string()
}

impl Flow {
    fn admin_url(&self, path: &str) -> String {
        format!("{ADMIN}/orgs/{}/{path}", self.w.org_a)
    }

    /// The factory's own purchase record (a spreadsheet import) for `mpn` at `price` yen.
    async fn past_purchase(&self, on: &str, mpn: &str, price: u32) {
        self.import_purchase(&self.w.admin_a, on, mpn, price, "pcs")
            .await;
    }

    /// A purchase record for `mpn` at `price` yen per `unit`, imported by `token`.
    async fn import_purchase(&self, token: &str, on: &str, mpn: &str, price: u32, unit: &str) {
        let csv = format!(
            "date,mpn,description,quantity,unit,unit_price\n{on},{mpn},Bearing,100,{unit},{price}\n"
        );
        let r = self
            .w
            .app
            .post(
                &self.url("purchase-history/imports"),
                token,
                json!({
                    "file_name": "past.csv",
                    "data_base64": base64::engine::general_purpose::STANDARD.encode(csv),
                    "header_row": 0,
                    "mapping": ["date", "mpn", "description", "quantity", "unit", "unit_price"],
                }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    }

    async fn reconcile(&self, o: &Ordered) {
        self.deliver(
            o,
            "2026-10-05T00:00:00Z",
            json!([{ "order_item_id": o.items[0], "quantity": 100 },
                   { "order_item_id": o.items[1], "quantity": 4 }]),
        )
        .await;
        let r = self.invoice(o, 1200, 30_200).await;
        assert_eq!(r.body["match_status"], "matched", "{:?}", r.body);
    }
}

use base64::Engine;

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn reconciled_orders_earn_half_of_the_saving(pool: PgPool) {
    let f = Flow::new(pool).await;
    let app = &f.w.app;
    let admin = admin_session(app).await;

    // 50% for 36 months from 1 October 2026 (defaults).
    let r = app
        .put(
            &f.admin_url("savings/agreement"),
            &admin,
            json!({ "starts_on": "2026-10-01" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);
    let r = app
        .put(
            &f.admin_url("savings/agreement"),
            &admin,
            json!({ "starts_on": "2026-10-01", "share_bps": 0 }),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);

    let o = f.ordered("PO-S-1", false, "2026-10-01T15:30:00Z").await;
    // The bearing: paid 1,500 before the agreement (and 1,100 in an older record, which is
    // not the price paid last); a record after the start does not count either.
    f.past_purchase("2026-03-01", &o.mpns[0], 1100).await;
    f.past_purchase("2026-09-01", &o.mpns[0], 1500).await;
    f.past_purchase("2026-10-01", &o.mpns[0], 900).await;
    // The guide has no record: agreed with the factory at 50,000 a set.
    let r = app
        .post(
            &f.admin_url("savings/baselines"),
            &admin,
            json!({ "mpn": o.mpns[1], "unit_price_milli": 50_000_000, "unit": "set",
                    "note": "Factory invoice 2026-02 no. 77" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let agreed = uuid(&r.body["id"]);
    // Only a factory approver confirms it; not our operator, not a requester.
    let confirm = f.url(&format!("savings/baselines/{agreed}/confirm"));
    assert_eq!(
        app.post(&confirm, &f.operator, json!({})).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.post(&confirm, &f.requester, json!({})).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.post(&confirm, &f.manager, json!({})).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.post(&confirm, &f.manager, json!({})).await.status,
        StatusCode::CONFLICT
    );
    // A confirmed baseline stays.
    let r = app
        .delete(&f.admin_url(&format!("savings/baselines/{agreed}")), &admin)
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);

    f.reconcile(&o).await;

    // Bearing: (1,500 − 1,200) × 100 = 30,000 saved, fee 15,000.
    // Guide: (50,000 − 45,000) × 4 = 20,000 saved, fee 10,000.
    let r = app.get(&f.url("savings"), &f.finance).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let v = r.body;
    assert_eq!(v["agreement"]["share_bps"], 5000);
    assert_eq!(v["agreement"]["term_months"], 36);
    assert_eq!(v["agreement"]["ends_on"], "2029-09-30");
    let entries = v["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "{entries:?}");
    let bearing = entries
        .iter()
        .find(|e| e["unit_price_minor"] == 1200)
        .unwrap();
    assert_eq!(bearing["status"], "counted");
    assert_eq!(bearing["baseline_unit_price_milli"], 1_500_000);
    assert_eq!(bearing["saving_minor"], 30_000);
    assert_eq!(bearing["fee_minor"], 15_000);
    assert_eq!(bearing["ordered_on"], "2026-10-02");
    let guide = entries
        .iter()
        .find(|e| e["unit_price_minor"] == 45_000)
        .unwrap();
    assert_eq!(guide["status"], "counted");
    assert_eq!(guide["fee_minor"], 10_000);
    assert_eq!(
        v["totals"],
        json!([{ "currency": "JPY", "saving_minor": 50_000, "fee_minor": 25_000,
                 "unbilled_fee_minor": 25_000 }])
    );
    let history = v["baselines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["source"] == "history")
        .unwrap();
    assert_eq!(history["purchased_on"], "2026-09-01");
    assert_eq!(history["used"], 1);
    // A requester sees no money.
    assert_eq!(
        app.get(&f.url("savings"), &f.requester).await.status,
        StatusCode::FORBIDDEN
    );

    // Terms are fixed once fees were computed under them.
    let r = app
        .put(
            &f.admin_url("savings/agreement"),
            &admin,
            json!({ "starts_on": "2026-10-01", "share_bps": 6000 }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);

    // The second order of the same bearing and guide (written differently, confirmed as the
    // same items), at the same prices, is measured against the fixed baselines (1,500 and
    // 50,000), not against our first order.
    let o2 = f.ordered("PO-S-2", false, "2026-10-10T03:00:00Z").await;
    for (alias, canonical) in o2.mpns.iter().zip(&o.mpns) {
        let key = |m: &str| format!("m:{}", m.replace('-', "").to_uppercase());
        let r = app
            .post(
                &f.url("item-aliases"),
                &f.w.admin_a,
                json!({ "alias_key": key(alias), "canonical_key": key(canonical) }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    }
    f.reconcile(&o2).await;
    let v = app.get(&f.url("savings"), &f.finance).await.body;
    assert_eq!(v["totals"][0]["fee_minor"], 50_000, "{v:?}");

    // Billing: one draft line per order, 10% tax on the fee; then nothing is left to bill.
    let r = app
        .post(
            &f.admin_url("savings/invoice"),
            &admin,
            json!({ "through": "2026-10-31" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let inv = uuid(&r.body["id"]);
    let d = app
        .get(&f.admin_url(&format!("billing/invoices/{inv}")), &admin)
        .await;
    assert_eq!(d.status, StatusCode::OK, "{:?}", d.body);
    assert_eq!(d.body["subtotal_minor"], 50_000, "{:?}", d.body);
    assert_eq!(d.body["tax_minor"], 5_000);
    assert_eq!(d.body["lines"].as_array().unwrap().len(), 2);
    let again = app
        .post(
            &f.admin_url("savings/invoice"),
            &admin,
            json!({ "through": "2026-10-31" }),
        )
        .await;
    assert_eq!(again.status, StatusCode::CONFLICT);
    // Deleting the draft puts the fees back to unbilled.
    let r = app
        .delete(&f.admin_url(&format!("billing/invoices/{inv}")), &admin)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);
    let v = app.get(&f.url("savings"), &f.finance).await.body;
    assert_eq!(v["totals"][0]["unbilled_fee_minor"], 50_000);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn no_agreement_or_no_drop_means_no_fee(pool: PgPool) {
    let f = Flow::new(pool).await;
    let app = &f.w.app;
    // Without an agreement nothing is owed.
    let o = f.ordered("PO-N-1", false, "2026-10-01T15:30:00Z").await;
    f.past_purchase("2026-09-01", &o.mpns[0], 1500).await;
    f.reconcile(&o).await;
    let v = app.get(&f.url("savings"), &f.finance).await.body;
    assert!(v["agreement"].is_null());
    assert!(
        v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["status"] == "no_agreement" && e["fee_minor"] == 0)
    );
    assert_eq!(v["totals"], json!([]));

    let admin = admin_session(app).await;
    let r = app
        .put(
            &f.admin_url("savings/agreement"),
            &admin,
            json!({ "starts_on": "2026-10-01", "term_months": 36 }),
        )
        .await;
    // Lines recorded without terms never earn a fee and do not fix the terms.
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);

    // Ledger rows cannot be edited even past the API.
    let mut tx = f.owner.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.org_id', $1, true)")
        .bind(f.w.org_a.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.scope', 'platform', true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    let e = sqlx::query("UPDATE savings_entries SET fee_minor = 1, status = 'counted'")
        .execute(&mut *tx)
        .await;
    assert!(e.is_err());
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn price_at_or_above_the_baseline_and_unconfirmed_baselines_earn_nothing(pool: PgPool) {
    let f = Flow::new(pool).await;
    let app = &f.w.app;
    let admin = admin_session(app).await;
    let r = app
        .put(
            &f.admin_url("savings/agreement"),
            &admin,
            json!({ "starts_on": "2026-10-01" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let o = f.ordered("PO-Z-1", false, "2026-10-01T15:30:00Z").await;
    // Paid 1,000 before: the new 1,200 is dearer, so 0 — never negative.
    f.past_purchase("2026-09-01", &o.mpns[0], 1000).await;
    // Proposed for the guide but never confirmed by the factory.
    let r = app
        .post(
            &f.admin_url("savings/baselines"),
            &admin,
            json!({ "mpn": o.mpns[1], "unit_price_milli": 60_000_000, "unit": "set",
                    "note": "Phone call" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    f.reconcile(&o).await;
    let v = app.get(&f.url("savings"), &f.finance).await.body;
    let statuses: Vec<&str> = v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["status"].as_str().unwrap())
        .collect();
    assert!(statuses.contains(&"no_saving") && statuses.contains(&"unconfirmed_baseline"));
    assert_eq!(v["totals"], json!([]));
    let r = app
        .post(
            &f.admin_url("savings/invoice"),
            &admin,
            json!({ "through": "2026-12-31" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
}

// ---------------------------------------------------------------------------------------
// After approval: the delivery date is followed up; supplier replies are read from files.

impl Flow {
    async fn delivery_tasks(&self) -> Vec<Value> {
        let q = self.w.app.get("/api/v1/ops/queue", &self.operator).await;
        assert_eq!(q.status, StatusCode::OK, "{:?}", q.body);
        q.body["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["kind"] == "check_delivery")
            .cloned()
            .collect()
    }

    async fn order_header(&self, o: &Ordered) -> Value {
        let d = self
            .w
            .app
            .get(
                &self.url(&format!("requests/{}/order", o.request)),
                &self.manager,
            )
            .await;
        assert_eq!(d.status, StatusCode::OK, "{:?}", d.body);
        d.body["order"].clone()
    }
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn delivery_dates_are_followed_up_until_everything_arrives(pool: PgPool) {
    let f = Flow::new(pool).await;
    let app = &f.w.app;
    // Lead times quoted: 7 and 21 days → promised 21 days after the order.
    let o = f.ordered("PO-D-1", false, "2026-10-01T15:30:00Z").await;
    let h = f.order_header(&o).await;
    let promised = h["promised_on"].as_str().unwrap().to_string();
    let today = chrono::Utc::now()
        .with_timezone(&chrono::FixedOffset::east_opt(9 * 3600).unwrap())
        .date_naive();
    assert_eq!(promised, (today + chrono::Duration::days(21)).to_string());
    assert_eq!(h["expected_on"], promised.as_str());
    assert_eq!(h["late"], false);
    let tasks = f.delivery_tasks().await;
    assert_eq!(tasks.len(), 1, "{tasks:?}");
    assert_eq!(tasks[0]["title"], "Check delivery of PO PO-D-1");

    // Only our staff record the supplier's confirmed date.
    let url = f.url(&format!("orders/{}/delivery-date", o.order));
    let body = json!({ "confirmed_on": "2026-10-20", "note": "Tanaka-san, by phone" });
    assert_eq!(
        app.post(&url, &f.manager, body.clone()).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.post(&url, &f.operator, body).await.status,
        StatusCode::NO_CONTENT
    );
    let h = f.order_header(&o).await;
    assert_eq!(h["delivery_confirmed_on"], "2026-10-20");
    assert_eq!(h["delivery_note"], "Tanaka-san, by phone");
    assert_eq!(h["expected_on"], "2026-10-20");
    // The confirmed day has passed and nothing arrived: late, and the task is overdue.
    assert_eq!(h["late"], true);
    let tasks = f.delivery_tasks().await;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["overdue"], true);

    // Everything arrives: the follow-up closes and the order is no longer late.
    f.deliver(
        &o,
        "2026-10-21T00:00:00Z",
        json!([{ "order_item_id": o.items[0], "quantity": 100 },
               { "order_item_id": o.items[1], "quantity": 4 }]),
    )
    .await;
    assert!(f.delivery_tasks().await.is_empty());
    assert_eq!(f.order_header(&o).await["late"], false);
    assert_eq!(
        app.post(&url, &f.operator, json!({ "confirmed_on": "2026-10-25" }))
            .await
            .status,
        StatusCode::CONFLICT
    );
    let (events, chain_valid) = f.audit_actions().await;
    assert!(chain_valid);
    assert!(
        events
            .iter()
            .any(|e| e["action"] == "order.delivery_date_confirmed")
    );
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn supplier_reply_files_are_read_as_text(pool: PgPool) {
    let f = Flow::new(pool).await;
    let o = f.ordered("PO-R-1", false, "2026-10-01T15:30:00Z").await;
    let eml = "From: Sales <sales@tokyo-bearings.test>\r\nSubject: Quote\r\n\
               Content-Type: text/plain; charset=UTF-8\r\n\r\n6204ZZ 100個 単価 1,180円 納期 7日\r\n";
    let body = |name: &str, data: &[u8]| {
        json!({ "file_name": name,
                "data_base64": base64::engine::general_purpose::STANDARD.encode(data) })
    };
    let url = f.url(&format!("rfqs/{}/reply-text", o.rfq));
    let r =
        f.w.app
            .post(&url, &f.operator, body("reply.eml", eml.as_bytes()))
            .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let text = r.body["text"].as_str().unwrap();
    assert!(
        text.contains("From: Sales <sales@tokyo-bearings.test>"),
        "{text}"
    );
    assert!(text.contains("単価 1,180円"), "{text}");
    // The file is kept as received (電子帳簿保存法) and can be fetched back unchanged.
    let doc = uuid(&r.body["document_id"]);
    let (status, _, bytes) =
        f.w.app
            .get_raw(
                &f.url(&format!("supplier-documents/{doc}/content")),
                &f.manager,
            )
            .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, eml.as_bytes());
    let sha = hex::encode(sha2::Sha256::digest(eml.as_bytes()));
    // The database refuses to change or delete it, even for the server itself.
    let mut tx = f.owner.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE procure_runtime")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.org_id', $1, true)")
        .bind(f.w.org_a.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let e = sqlx::query("UPDATE supplier_documents SET file_name = 'x' WHERE id = $1")
        .bind(doc)
        .execute(&mut *tx)
        .await;
    assert!(e.is_err(), "update must be refused");
    tx.rollback().await.unwrap();
    let mut tx = f.owner.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE procure_runtime")
        .execute(&mut *tx)
        .await
        .unwrap();
    let e = sqlx::query("DELETE FROM supplier_documents WHERE id = $1")
        .bind(doc)
        .execute(&mut *tx)
        .await;
    assert!(e.is_err(), "delete must be refused");
    tx.rollback().await.unwrap();
    // A supplier not asked in this RFQ cannot be named as the sender.
    let mut other = body("reply.eml", eml.as_bytes());
    other["supplier_id"] = json!(Uuid::now_v7());
    assert_eq!(
        f.w.app.post(&url, &f.operator, other).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // The records register: the quotation (linked to its file) and the order, by date,
    // amount and counterparty.
    f.as_owner(
        "UPDATE quotations SET source_document_id = $1 WHERE rfq_id = $2::uuid",
        doc,
        &o.rfq.to_string(),
    )
    .await;
    let fr = &f;
    let rec = |q: &str| {
        let url = fr.url(&format!("records{q}"));
        async move { fr.w.app.get(&url, &fr.manager).await }
    };
    let r = rec("").await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let rows = r.body["rows"].as_array().unwrap();
    let kinds: Vec<&str> = rows.iter().map(|r| r["kind"].as_str().unwrap()).collect();
    assert!(
        kinds.contains(&"quotation") && kinds.contains(&"order"),
        "{rows:?}"
    );
    let quote = rows.iter().find(|r| r["kind"] == "quotation").unwrap();
    // 1,200 × 100 + 45,000 × 4 + 2,000 shipping.
    assert_eq!(quote["amount_minor"], 120_000 + 180_000 + 2_000);
    assert_eq!(quote["counterparty"], "Tokyo Bearings KK");
    assert_eq!(quote["document_sha256"], sha.as_str());
    let order = rows.iter().find(|r| r["kind"] == "order").unwrap();
    assert_eq!(order["reference"], "PO-R-1");
    assert_eq!(order["date"], "2026-10-02"); // 15:30 UTC is the next day in Tokyo
    let n = |r: &Resp| r.body["rows"].as_array().unwrap().len();
    assert_eq!(n(&rec("?counterparty=tokyo%20bear&kind=order").await), 1);
    assert_eq!(n(&rec("?counterparty=osaka").await), 0);
    assert_eq!(
        n(&rec("?from=2026-10-02&to=2026-10-02&kind=order").await),
        1
    );
    assert_eq!(n(&rec("?to=2026-10-01&kind=order").await), 0);
    // The quotation and the order it became both total ¥302,000.
    assert_eq!(n(&rec("?min_minor=302000&max_minor=302000").await), 2);
    assert_eq!(n(&rec("?min_minor=302001").await), 0);
    assert_eq!(
        rec("?kind=receipt").await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        f.w.app.get(&f.url("records"), &f.requester).await.status,
        StatusCode::FORBIDDEN
    );
    // Customers do not handle supplier replies; unknown formats are refused.
    assert_eq!(
        f.w.app
            .post(&url, &f.manager, body("reply.eml", eml.as_bytes()))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let r =
        f.w.app
            .post(&url, &f.operator, body("x.bin", &[0, 1, 2]))
            .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn baselines_our_staff_imported_wait_for_the_factory_then_price_the_waiting_lines(
    pool: PgPool,
) {
    let f = Flow::new(pool).await;
    let app = &f.w.app;
    let admin = admin_session(app).await;
    let r = app
        .put(
            &f.admin_url("savings/agreement"),
            &admin,
            json!({ "starts_on": "2026-10-01", "share_bps": 1250 }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let o = f.ordered("PO-W-1", false, "2026-10-01T15:30:00Z").await;
    // Our operator onboarded the bearing's record (1,500): it is not the factory's word yet.
    f.import_purchase(&f.operator, "2026-09-01", &o.mpns[0], 1500, "pcs")
        .await;
    // The factory's own record for the guide has no unit: it waits for confirmation too.
    f.import_purchase(&f.w.admin_a, "2026-09-01", &o.mpns[1], 50_000, "")
        .await;
    f.reconcile(&o).await;
    let v = app.get(&f.url("savings"), &f.finance).await.body;
    assert!(
        v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["status"] == "unconfirmed_baseline" && e["fee_minor"] == 0),
        "{v:?}"
    );
    assert_eq!(v["totals"], json!([]));

    // The factory confirms both; the waiting lines are priced at 12.5 %:
    // (1,500 − 1,200) × 100 = 30,000 → 3,750; (50,000 − 45,000) × 4 = 20,000 → 2,500.
    for b in v["baselines"].as_array().unwrap() {
        let r = app
            .post(
                &f.url(&format!(
                    "savings/baselines/{}/confirm",
                    b["id"].as_str().unwrap()
                )),
                &f.manager,
                json!({}),
            )
            .await;
        assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);
    }
    let v = app.get(&f.url("savings"), &f.finance).await.body;
    assert_eq!(
        v["totals"],
        json!([{ "currency": "JPY", "saving_minor": 50_000, "fee_minor": 6_250,
                 "unbilled_fee_minor": 6_250 }]),
        "{v:?}"
    );

    // Baselines are fixed: no price edit past the API, and a confirmed one is never deleted.
    let mut tx = f.owner.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.org_id', $1, true)")
        .bind(f.w.org_a.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let e = sqlx::query("UPDATE savings_baselines SET unit_price_milli = 9_000_000")
        .execute(&mut *tx)
        .await;
    assert!(e.is_err());
    drop(tx);

    // The draft is the console's business: the factory sees the fee as unbilled until issued.
    let r = app
        .post(
            &f.admin_url("savings/invoice"),
            &admin,
            json!({ "through": "2026-10-31" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let v = app.get(&f.url("savings"), &f.finance).await.body;
    assert!(
        v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["invoice_id"].is_null())
    );
    let a = app.get(&f.admin_url("savings"), &admin).await.body;
    assert!(
        a["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["invoice_id"].is_string())
    );
    let inv = uuid(&r.body["id"]);
    let d = app
        .get(&f.admin_url(&format!("billing/invoices/{inv}")), &admin)
        .await;
    assert!(
        d.body["lines"][0]["description"]
            .as_str()
            .unwrap()
            .starts_with("Savings share 12.5% — PO PO-W-1"),
        "{:?}",
        d.body
    );
}

impl Flow {
    /// A directory supplier, verified unless `verified` is false.
    async fn supplier(&self, name: &str, categories: Value, verified: bool) -> Uuid {
        let r = self
            .w
            .app
            .post(
                "/api/v1/suppliers",
                &self.operator,
                json!({ "name": name, "country": "jp", "categories": categories }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let s = uuid(&r.body["id"]);
        if verified {
            self.ok(self
                .w
                .app
                .post(
                    &format!("/api/v1/suppliers/{s}/status"),
                    &self.operator,
                    json!({ "status": "verified" }),
                )
                .await);
        }
        s
    }
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn the_shortlist_ranks_suppliers_from_the_factorys_own_record(pool: PgPool) {
    let f = Flow::new(pool).await;
    let app = &f.w.app;
    // Supplier A quoted and got the order for the bearing earlier.
    let earlier = f
        .ordered("PO-S-1", false, "2026-10-01T09:00:00+09:00")
        .await;
    let denki = f.supplier("Denki Shoji", json!(["electrical"]), true).await;
    let kogu = f.supplier("Kogu Ya", json!(["tools"]), true).await;
    let unverified = f
        .supplier("Unverified Bearing", json!(["bearings"]), false)
        .await;
    let rejected = f
        .supplier("Rejected Bearing", json!(["bearings"]), true)
        .await;
    f.ok(f
        .op(&format!("suppliers/{rejected}/proposal"), json!({}))
        .await);
    f.ok(app
        .put(
            &f.url(&format!("suppliers/{rejected}/decision")),
            &f.manager,
            json!({ "decision": "rejected", "reason": "Late deliveries" }),
        )
        .await);

    let r = app
        .post(
            &f.url("requests"),
            &f.requester,
            json!({
                "title": "補充", "currency": "JPY", "needed_by": "2026-12-15",
                "items": [
                    { "description": "Ball bearing", "mpn": earlier.mpns[0], "quantity": 50 },
                    { "description": "ＶＶＦケーブル 2.0mm 100m", "quantity": 3 }
                ]
            }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let request = uuid(&r.body["id"]);
    f.ok(app
        .post(
            &f.url(&format!("requests/{request}/submit")),
            &f.requester,
            json!({}),
        )
        .await);
    f.ok(f
        .op(&format!("requests/{request}/pick-up"), json!({}))
        .await);
    let path = f.url(&format!("requests/{request}/supplier-shortlist"));

    let r = app.get(&path, &f.operator).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let picks = r.body["picks"].as_array().unwrap();
    let ids: Vec<Uuid> = picks.iter().map(|p| uuid(&p["supplier_id"])).collect();
    // Rejected by the factory, or not verified: never listed.
    assert_eq!(ids, vec![f.supplier, denki, kogu], "{picks:?}");
    assert!(!ids.contains(&unverified) && !ids.contains(&rejected));
    let codes = |p: &Value| -> Vec<String> {
        p["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["code"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        codes(&picks[0]),
        vec![
            "quoted_before",
            "ordered_before",
            "category",
            "approved_by_factory"
        ]
    );
    assert_eq!(picks[0]["score"], 30 + 6 + 12 + 10);
    assert_eq!(picks[0]["reasons"][0]["count"], 1);
    assert_eq!(picks[1]["reasons"][0]["category"], "electrical");
    assert_eq!(
        picks
            .iter()
            .map(|p| p["selected"].as_bool().unwrap())
            .collect::<Vec<_>>(),
        vec![true, true, false]
    );
    assert_eq!(picks[2]["relevant"], false);

    // A supplier already asked for a request is not selected again for it.
    let r = app
        .get(
            &f.url(&format!(
                "requests/{}/supplier-shortlist?picks=1",
                earlier.request
            )),
            &f.operator,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let picks = r.body["picks"].as_array().unwrap();
    let asked = picks
        .iter()
        .find(|p| uuid(&p["supplier_id"]) == f.supplier)
        .unwrap();
    assert_eq!(asked["selected"], false);
    assert!(codes(asked).contains(&"already_invited".to_string()));
    // Its own quotation for that request does not count as "quoted before".
    assert!(!codes(asked).contains(&"quoted_before".to_string()));
    assert!(picks.iter().filter(|p| p["selected"] == true).count() <= 1);

    // Only our assigned staff use it; the factory's own users do not.
    for token in [&f.requester, &f.manager] {
        assert_eq!(app.get(&path, token).await.status, StatusCode::FORBIDDEN);
    }
    assert_eq!(
        app.get(&format!("{path}?picks=0"), &f.operator)
            .await
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        app.get(
            &f.url(&format!("requests/{}/supplier-shortlist", Uuid::now_v7())),
            &f.operator
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn opportunities_show_what_was_paid_above_the_factorys_own_lowest_price(pool: PgPool) {
    let f = Flow::new(pool).await;
    let app = &f.w.app;
    let today = chrono::Utc::now().date_naive();
    let d = |days: i64| (today - chrono::Duration::days(days)).to_string();
    let csv = format!(
        "date,supplier,mpn,description,quantity,unit,unit_price\n\
         {},Denki Shoji,VVF-2.0,VVF cable 100m,10,pcs,12000\n\
         {},Kanto Cable,VVF-2.0,VVF cable 100m,20,pcs,10000\n\
         {},Denki Shoji,VVF-2.0,VVF cable 100m,5,pcs,11000\n\
         {},Kogu Ya,EM-6,End mill 6mm,4,pcs,3000\n\
         {},Kogu Ya,EM-6,End mill 6mm,4,pcs,3000\n\
         {},Old Supplier,OLD-1,Bought two years ago,1,pcs,999999\n",
        d(30),
        d(60),
        d(90),
        d(10),
        d(40),
        d(800)
    );
    let r = app
        .post(
            &f.url("purchase-history/imports"),
            &f.w.admin_a,
            json!({
                "file_name": "past.csv",
                "data_base64": base64::engine::general_purpose::STANDARD.encode(csv),
                "header_row": 0,
                "mapping": ["date", "supplier", "mpn", "description", "quantity", "unit", "unit_price"],
            }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);

    let path = f.url("purchase-history/opportunities");
    let r = app.get(&path, &f.w.admin_a).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["share_bps"], 5000);
    let items = r.body["items"].as_array().unwrap();
    // The two-year-old purchase is outside the 12 months.
    assert_eq!(items.len(), 2, "{items:?}");
    let cable = &items[0];
    assert_eq!(cable["item_key"], "m:VVF20");
    assert_eq!(cable["spend_minor"], 120_000 + 200_000 + 55_000);
    // (12000−10000)×10 + (11000−10000)×5
    assert_eq!(cable["potential_minor"], 25_000);
    assert_eq!(cable["fee_minor"], 12_500);
    assert_eq!(cable["suppliers"], 2);
    assert_eq!(items[1]["single_supplier"], true);
    assert_eq!(items[1]["potential_minor"], 0);
    assert_eq!(r.body["totals"][0]["potential_minor"], 25_000);
    assert_eq!(r.body["totals"][0]["items"], 2);
    let r = app.get(&format!("{path}?months=36"), &f.w.admin_a).await;
    assert_eq!(r.body["items"].as_array().unwrap().len(), 3);
    assert_eq!(
        app.get(&format!("{path}?months=0"), &f.w.admin_a)
            .await
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // Our assigned staff read it too; a requester does not.
    assert_eq!(app.get(&path, &f.operator).await.status, StatusCode::OK);
    assert_eq!(
        app.get(&path, &f.requester).await.status,
        StatusCode::FORBIDDEN
    );
}

/// Records what would go to the SMTP server; can be made to fail.
#[derive(Default)]
struct FakeSmtp {
    sent: std::sync::Mutex<Vec<(String, Vec<u8>)>>,
    fail: std::sync::atomic::AtomicBool,
}

impl procure_server::supplier_mail::Transport for FakeSmtp {
    fn send<'a>(
        &'a self,
        _from: &'a str,
        to: &'a str,
        raw: &'a [u8],
    ) -> futures_util::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
                return Err("421 try again later".into());
            }
            self.sent
                .lock()
                .unwrap()
                .push((to.to_string(), raw.to_vec()));
            Ok(())
        })
    }
}

/// (UID, size, content) — content `None` as if it was not downloaded.
type FakeMessage = (u32, u64, Option<Vec<u8>>);

#[derive(Default)]
struct FakeMailbox {
    messages: std::sync::Mutex<Vec<FakeMessage>>,
}

type FetchResult = Result<(u32, Vec<procure_server::supplier_mail::Fetched>), String>;

impl procure_server::supplier_mail::Inbox for FakeMailbox {
    fn fetch<'a>(
        &'a self,
        from_uid: u32,
        _since: chrono::NaiveDate,
    ) -> futures_util::future::BoxFuture<'a, FetchResult> {
        Box::pin(async move {
            let all = self.messages.lock().unwrap().clone();
            Ok((
                7,
                all.into_iter()
                    .filter(|(u, _, _)| *u >= from_uid)
                    .map(|(uid, size, raw)| procure_server::supplier_mail::Fetched {
                        uid,
                        size,
                        raw,
                    })
                    .collect(),
            ))
        })
    }
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn live_mail_sends_retries_reminds_and_reads_replies_and_faxes(pool: PgPool) {
    use procure_server::supplier_mail::{SupplierMail, poll_inbox, send_queued};
    let smtp = std::sync::Arc::new(FakeSmtp::default());
    let mailbox = std::sync::Arc::new(FakeMailbox::default());
    let mail = std::sync::Arc::new(SupplierMail {
        from: "Chuo-Sogo 購買 <rfq@chuo-sogo.test>".into(),
        reply_to: None,
        fax_gateway: Some("{number}@fax-gw.test".into()),
        fax_senders: vec!["@fax-in.test".into()],
        allow_domains: vec![],
        transport: smtp.clone(),
        inbox: Some(mailbox.clone()),
        imap: Some(procure_server::supplier_mail::ImapConfig {
            host: "imap.test".into(),
            port: 993,
            user: "u".into(),
            password: "p".into(),
            mailbox: "INBOX".into(),
            poll: std::time::Duration::from_secs(60),
            since: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
        }),
    });
    let owner = pool.clone();
    let m = mail.clone();
    let app = App::configured(pool, move |s| s.supplier_mail = Some(m)).await;
    let f = Flow::with_world(owner, world_with(app).await).await;
    let state = f.w.app.state.clone();
    let (request, rfq, items, _) = f.sent_rfq().await;

    // The SMTP server refuses at first: the message stays queued with the error, then goes.
    smtp.fail.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(send_queued(&state, &mail).await.unwrap(), 0);
    let fr = &f;
    let outbox = || {
        let url = fr.url(&format!("rfqs/{rfq}/outbox"));
        async move { fr.w.app.get(&url, &fr.operator).await.body }
    };
    let o = outbox().await;
    assert_eq!(o[0]["status"], "queued", "{o:?}");
    assert_eq!(o[0]["channel"], "email");
    assert!(o[0]["last_error"].as_str().unwrap().contains("421"));
    f.as_owner(
        "UPDATE outbox SET next_attempt_at = now() - $2::interval
         WHERE rfq_recipient_id IN (SELECT id FROM rfq_recipients WHERE rfq_id = $1)",
        rfq,
        "1 second",
    )
    .await;
    smtp.fail.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(send_queued(&state, &mail).await.unwrap(), 1);
    let o = outbox().await;
    assert_eq!(o[0]["status"], "sent");
    let subject = o[0]["subject"].as_str().unwrap().to_string();
    let (to, raw) = smtp.sent.lock().unwrap()[0].clone();
    assert_eq!(to, "rfq@tokyo-bearings.test");
    let raw = String::from_utf8_lossy(&raw).to_string();
    assert!(raw.contains("Message-ID: <"), "{raw}");
    let token = procure_server::supplier_mail::token_in(&subject).unwrap();

    // One reminder to those who have not replied; not again within 20 hours.
    let remind = f.url(&format!("rfqs/{rfq}/remind"));
    let r = f.w.app.post(&remind, &f.operator, json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(
        (r.body["channel"].clone(), r.body["messages"].clone()),
        (json!("live"), json!(1))
    );
    let r = f.w.app.post(&remind, &f.operator, json!({})).await;
    assert_eq!(r.body["error"]["detail"], "reminded_recently");
    assert_eq!(send_queued(&state, &mail).await.unwrap(), 1);
    assert_eq!(smtp.sent.lock().unwrap().len(), 2);

    // Replies: one answering our subject, one FAX that cannot say which RFQ it is for.
    let reply = format!(
        "From: Sales <sales@tokyo-bearings.test>\r\nSubject: Re: {subject}\r\n\
         Date: Tue, 3 Nov 2026 10:00:00 +0900\r\nContent-Type: text/plain; charset=UTF-8\r\n\r\n\
         6204ZZ 100個 単価 1,150円\r\n"
    );
    let fax = "From: fax@fax-in.test\r\nSubject: FAX from 0312345678\r\n\
               Content-Type: text/plain; charset=UTF-8\r\n\r\nHSR15 4 set 44,000\r\n";
    // Someone else who saw the subject (a forward, a distributor) cannot file a quotation
    // in the supplier's name: same token, other domain, so it waits for a person.
    let forged = format!(
        "From: someone@other-domain.test\r\nSubject: Re: {subject}\r\n\
         Content-Type: text/plain; charset=UTF-8\r\n\r\n6204ZZ 1円\r\n"
    );
    let sized = |b: &[u8]| (b.len() as u64, Some(b.to_vec()));
    let (rs, rr) = sized(reply.as_bytes());
    let (fs, fr) = sized(fax.as_bytes());
    let (gs, gr) = sized(forged.as_bytes());
    mailbox.messages.lock().unwrap().extend([
        // Too large: recorded, never downloaded, never fetched again.
        (4, 20 * 1024 * 1024, None),
        (5, rs, rr),
        (6, fs, fr),
        (7, gs, gr),
    ]);
    assert_eq!(poll_inbox(&state, &mail).await.unwrap(), 1);
    // Read again: nothing new.
    assert_eq!(poll_inbox(&state, &mail).await.unwrap(), 0);
    let detail =
        f.w.app
            .get(&f.url(&format!("rfqs/{rfq}")), &f.operator)
            .await;
    let docs = detail.body["documents"].as_array().unwrap().clone();
    assert_eq!(docs.len(), 1, "{docs:?}");
    assert_eq!(docs[0]["channel"], "email");
    assert_eq!(uuid(&docs[0]["supplier_id"]), f.supplier);
    // Customers never see what suppliers sent before our staff checked it.
    let customer =
        f.w.app
            .get(&f.url(&format!("rfqs/{rfq}")), &f.manager)
            .await;
    assert_eq!(customer.body["documents"], json!([]));
    let tasks =
        f.w.app
            .get(&f.url(&format!("requests/{request}/tasks")), &f.operator)
            .await;
    assert!(
        tasks
            .body
            .to_string()
            .contains("Reply received from Tokyo Bearings KK"),
        "{:?}",
        tasks.body
    );
    let email_doc = uuid(&docs[0]["id"]);
    let r = f
        .op(
            &format!("rfqs/{rfq}/supplier-documents/{email_doc}/text"),
            json!({}),
        )
        .await;
    assert!(
        r.body["text"].as_str().unwrap().contains("単価 1,150円"),
        "{:?}",
        r.body
    );
    let _ = token;

    // The FAX waits in our staff inbox; it is read, then assigned by a person.
    let inbox = f.w.app.get("/api/v1/mail-inbox", &f.operator).await;
    let list = inbox.body.as_array().unwrap().clone();
    assert_eq!(list.len(), 2, "{list:?}");
    assert!(
        list.iter()
            .any(|m| m["from_address"] == "someone@other-domain.test")
    );
    let fax_msg = list.iter().find(|m| m["channel"] == "fax").unwrap();
    let msg = uuid(&fax_msg["id"]);
    let t =
        f.w.app
            .get(&format!("/api/v1/mail-inbox/{msg}/text"), &f.operator)
            .await;
    assert!(
        t.body["text"].as_str().unwrap().contains("44,000"),
        "{:?}",
        t.body
    );
    assert_eq!(
        f.w.app.get("/api/v1/mail-inbox", &f.manager).await.status,
        StatusCode::FORBIDDEN
    );
    let assign = format!("/api/v1/mail-inbox/{msg}/assign");
    let r =
        f.w.app
            .post(
                &assign,
                &f.operator,
                json!({ "org_id": f.w.org_a, "rfq_id": rfq, "supplier_id": f.supplier }),
            )
            .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let r =
        f.w.app
            .post(
                &assign,
                &f.operator,
                json!({ "org_id": f.w.org_a, "rfq_id": rfq }),
            )
            .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let detail =
        f.w.app
            .get(&f.url(&format!("rfqs/{rfq}")), &f.operator)
            .await;
    let docs = detail.body["documents"].as_array().unwrap().clone();
    assert_eq!(docs.len(), 2);
    assert!(docs.iter().any(|d| d["channel"] == "fax"));
    let left = f.w.app.get("/api/v1/mail-inbox", &f.operator).await.body;
    assert_eq!(left.as_array().unwrap().len(), 1, "{left:?}");
    let forged_id = uuid(&left[0]["id"]);
    let r =
        f.w.app
            .post(
                &format!("/api/v1/mail-inbox/{forged_id}/set-aside"),
                &f.operator,
                json!({}),
            )
            .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(
        f.w.app.get("/api/v1/mail-inbox", &f.operator).await.body,
        json!([])
    );

    // The quotation is recorded from the email; then nobody is left to remind.
    let r = f
        .op(
            &format!("rfqs/{rfq}/quotations"),
            json!({
                "supplier_id": f.supplier, "currency": "JPY", "valid_until": "2099-01-01",
                "source_document_id": email_doc,
                "lines": [
                    { "request_item_id": items[0], "unit_price_minor": 1150, "quantity": 100 },
                    { "request_item_id": items[1], "unit_price_minor": 44_000, "quantity": 4 }
                ]
            }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let detail =
        f.w.app
            .get(&f.url(&format!("rfqs/{rfq}")), &f.operator)
            .await;
    let used = detail.body["documents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| uuid(&d["id"]) == email_doc)
        .unwrap()["used"]
        .clone();
    assert_eq!(used, true);
    f.as_owner(
        "UPDATE outbox SET created_at = now() - $2::interval
         WHERE kind = 'reminder'
           AND rfq_recipient_id IN (SELECT id FROM rfq_recipients WHERE rfq_id = $1)",
        rfq,
        "1 day",
    )
    .await;
    let r = f.w.app.post(&remind, &f.operator, json!({})).await;
    assert_eq!(
        r.body["error"]["detail"], "nobody_to_remind",
        "{:?}",
        r.body
    );

    let status = f.w.app.get("/api/v1/mail/status", &f.operator).await;
    assert_eq!(status.body["live"], true);
    assert_eq!(status.body["fax"], true);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn suppliers_can_take_requests_by_fax(pool: PgPool) {
    let f = Flow::new(pool).await;
    let url = format!("/api/v1/suppliers/{}/contact", f.supplier);
    let put = |body: Value| {
        let app = &f.w.app;
        let op = f.operator.clone();
        let url = url.clone();
        async move { app.put(&url, &op, body).await }
    };
    // FAX chosen without a number, or a number that is not one: refused.
    let r = put(json!({ "email": "rfq@tokyo-bearings.test", "rfq_channel": "fax" })).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    let r = put(json!({ "fax": "03-12ab", "rfq_channel": "fax" })).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    let r = put(
        json!({ "email": "rfq@tokyo-bearings.test", "fax": "03-1234-5678",
                        "rfq_channel": "fax" }),
    )
    .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);
    let list =
        f.w.app
            .get("/api/v1/suppliers?status=verified", &f.operator)
            .await;
    let s = list.body.as_array().unwrap()[0].clone();
    assert_eq!(
        (s["fax"].clone(), s["rfq_channel"].clone()),
        (json!("0312345678"), json!("fax"))
    );
    // Without live mail configured, a FAX supplier's request is only recorded, as before.
    let (_, rfq, _, _) = f.sent_rfq().await;
    let o =
        f.w.app
            .get(&f.url(&format!("rfqs/{rfq}/outbox")), &f.operator)
            .await;
    assert_eq!(o.body[0]["channel"], "test");
    assert_eq!(o.body[0]["status"], "recorded");
    assert_eq!(
        f.w.app
            .put(&url, &f.manager, json!({ "rfq_channel": "email" }))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}
