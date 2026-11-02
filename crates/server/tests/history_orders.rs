//! Orders completed in this system are recorded into the customer's purchase history when they
//! reconcile (matched invoice, or the last discrepancy resolved), against a real database.

// Test-only scenario helpers: a failed unwrap is a failed test, which is the intent.
#![allow(clippy::unwrap_used)]

mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};
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
    order: Uuid,
    items: Vec<Uuid>,
    mpns: Vec<String>,
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
    async fn ordered(&self, po: &str, offered_mpn: bool, ordered_at: &str) -> Ordered {
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
        self.ok(self.op(&format!("rfqs/{rfq}/dispatch"), json!({})).await);
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
    let c = call("/auth/totp/confirm", json!({ "token": token, "code": code })).await;
    assert_eq!(c.status, StatusCode::OK, "{:?}", c.body);
    c.body["token"].as_str().unwrap().to_string()
}

impl Flow {
    fn admin_url(&self, path: &str) -> String {
        format!("{ADMIN}/orgs/{}/{path}", self.w.org_a)
    }

    /// The factory's own purchase record (a spreadsheet import) for `mpn` at `price` yen.
    async fn past_purchase(&self, on: &str, mpn: &str, price: u32) {
        let csv = format!("date,mpn,description,quantity,unit_price\n{on},{mpn},Bearing,100,{price}\n");
        let r = self
            .w
            .app
            .post(
                &self.url("purchase-history/imports"),
                &self.w.admin_a,
                json!({
                    "file_name": "past.csv",
                    "data_base64": base64::engine::general_purpose::STANDARD.encode(csv),
                    "header_row": 0,
                    "mapping": ["date", "mpn", "description", "quantity", "unit_price"],
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
    let bearing = entries.iter().find(|e| e["unit_price_minor"] == 1200).unwrap();
    assert_eq!(bearing["status"], "counted");
    assert_eq!(bearing["baseline_unit_price_milli"], 1_500_000);
    assert_eq!(bearing["saving_minor"], 30_000);
    assert_eq!(bearing["fee_minor"], 15_000);
    assert_eq!(bearing["ordered_on"], "2026-10-02");
    let guide = entries.iter().find(|e| e["unit_price_minor"] == 45_000).unwrap();
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
    // Ledger rows exist (all without a fee), so the terms are already fixed.
    assert_eq!(r.status, StatusCode::CONFLICT);

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
