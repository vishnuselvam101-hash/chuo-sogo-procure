//! Quotation comparison against the customer's last purchase price: reference only, never a
//! change to compliance, ranking or any decision; hidden from readers without history access.

#![allow(clippy::unwrap_used)]

mod common;

use axum::http::StatusCode;
use base64::Engine;
use common::*;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

/// Past purchases. The bearing was bought twice (the later one counts), the guide once; the
/// verified part numbers below are typed differently and must still match.
const HISTORY: &str = "伝票日付,仕入先名,品名,型番,数量,単価（円）\n\
2025/01/10,山田商事,玉軸受,PC-6204ZZ,10,1000\n\
2026/03/01,鈴木機材,玉軸受,PC 6204 zz,10,1100\n\
2026/02/01,山田商事,リニアガイド,ＰＣ－ＨＳＲ１５,4,40000\n";

struct Setup {
    w: World,
    operator: String,
    requester: String,
    manager: String,
    suppliers: Vec<Uuid>,
}

impl Setup {
    fn url(&self, path: &str) -> String {
        format!("/api/v1/orgs/{}/{path}", self.w.org_a)
    }

    async fn op(&self, path: &str, body: Value) -> Value {
        let r = self.w.app.post(&self.url(path), &self.operator, body).await;
        assert!(r.status.is_success(), "{path}: {}: {:?}", r.status, r.body);
        r.body
    }

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
        let mut s = Setup {
            w,
            operator,
            requester,
            manager,
            suppliers: vec![],
        };
        for (name, email) in [
            ("Tokyo Bearings KK", "rfq@tokyo.test"),
            ("Osaka Motion Ltd", "sales@osaka.test"),
            ("Pacific Parts Inc", "sales@pacific.test"),
        ] {
            let r =
                s.w.app
                    .post(
                        "/api/v1/suppliers",
                        &s.operator,
                        json!({ "name": name, "country": "jp", "email": email,
                            "categories": ["bearings"] }),
                    )
                    .await;
            assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
            let id = uuid(&r.body["id"]);
            let r =
                s.w.app
                    .post(
                        &format!("/api/v1/suppliers/{id}/status"),
                        &s.operator,
                        json!({ "status": "verified" }),
                    )
                    .await;
            assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);
            s.op(&format!("suppliers/{id}/proposal"), json!({})).await;
            let r =
                s.w.app
                    .put(
                        &s.url(&format!("suppliers/{id}/decision")),
                        &s.manager,
                        json!({ "decision": "approved" }),
                    )
                    .await;
            assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);
            s.suppliers.push(id);
        }
        s
    }

    /// Request with two lines, both verified against catalogue records, RFQ sent to all three
    /// suppliers. Returns (rfq, [item ids]).
    async fn rfq(&self) -> (Uuid, Vec<Uuid>) {
        let r = self
            .w
            .app
            .post(
                &self.url("requests"),
                &self.requester,
                json!({
                    "title": "軸受補充",
                    "currency": "JPY",
                    "items": [
                        { "description": "Deep groove ball bearing", "mpn": "6204", "quantity": 100 },
                        { "description": "Linear guide", "mpn": "HSR", "quantity": 4 }
                    ]
                }),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let request = uuid(&r.body["id"]);
        let r = self
            .w
            .app
            .post(
                &self.url(&format!("requests/{request}/submit")),
                &self.requester,
                json!({}),
            )
            .await;
        assert!(r.status.is_success(), "{:?}", r.body);
        let detail = self
            .w
            .app
            .get(&self.url(&format!("requests/{request}")), &self.operator)
            .await;
        let items: Vec<Uuid> = detail.body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| uuid(&i["id"]))
            .collect();
        self.op(&format!("requests/{request}/pick-up"), json!({}))
            .await;
        for (item, mpn) in items.iter().zip(["PC-6204ZZ", "PC-HSR15"]) {
            let r = self
                .w
                .app
                .post(
                    "/api/v1/products",
                    &self.operator,
                    json!({ "manufacturer": "NSK", "mpn": mpn, "description": "Part",
                            "specs": {}, "evidence": [{ "kind": "datasheet",
                            "url": "https://example.test/ds.pdf" }] }),
                )
                .await;
            assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
            let product = uuid(&r.body["id"]);
            let r = self
                .w
                .app
                .post(
                    &format!("/api/v1/products/{product}/status"),
                    &self.operator,
                    json!({ "status": "verified" }),
                )
                .await;
            assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);
            self.op(
                &format!("requests/{request}/items/{item}/verification"),
                json!({ "product_id": product }),
            )
            .await;
        }
        self.op(&format!("requests/{request}/start-sourcing"), json!({}))
            .await;
        let r = self
            .op(
                &format!("requests/{request}/rfqs"),
                json!({ "supplier_ids": self.suppliers, "response_due": "2099-01-01",
                        "message": "Please quote." }),
            )
            .await;
        let rfq = uuid(&r["id"]);
        self.op(&format!("rfqs/{rfq}/dispatch"), json!({})).await;
        (rfq, items)
    }

    async fn quote(&self, rfq: Uuid, supplier: Uuid, currency: &str, items: &[Uuid], p: [i64; 2]) {
        self.op(
            &format!("rfqs/{rfq}/quotations"),
            json!({
                "supplier_id": supplier, "currency": currency, "shipping_minor": 0,
                "valid_until": "2099-01-01",
                "lines": [
                    { "request_item_id": items[0], "unit_price_minor": p[0], "quantity": 100,
                      "lead_time_days": 7 },
                    { "request_item_id": items[1], "unit_price_minor": p[1], "quantity": 4,
                      "lead_time_days": 14 }
                ]
            }),
        )
        .await;
    }
}

/// The decision fields of each result, by supplier.
fn decisions(results: &Value) -> Vec<Value> {
    let mut v: Vec<Value> = results
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            json!([
                r["supplier_id"],
                r["compliance"],
                r["reasons"],
                r["total_minor"],
                r["max_lead_time_days"],
                r["rank"]
            ])
        })
        .collect();
    v.sort_by_key(|x| x[0].as_str().unwrap().to_string());
    v
}

fn by_supplier(results: &Value, supplier: Uuid) -> &Value {
    results
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["supplier_id"] == json!(supplier))
        .unwrap()
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn quoted_lines_are_checked_against_the_last_price(pool: PgPool) {
    let s = Setup::new(pool).await;
    let app = &s.w.app;
    let (rfq, items) = s.rfq().await;
    let [s1, s2, s3] = [s.suppliers[0], s.suppliers[1], s.suppliers[2]];
    s.quote(rfq, s1, "JPY", &items, [1200, 45_000]).await;
    s.quote(rfq, s2, "JPY", &items, [1300, 38_000]).await;
    s.quote(rfq, s3, "USD", &items, [800, 30_000]).await;

    // Without history: no checks, nothing flagged.
    let before = s.op(&format!("rfqs/{rfq}/comparisons"), json!({})).await;
    for r in before["results"].as_array().unwrap() {
        assert_eq!(r["price_checks"], json!([]), "{r:?}");
        assert_eq!(r["price_alerts"], 0);
        assert_eq!(r["price_alert_bps"], Value::Null);
    }

    // The customer's records and alert line (+10 %).
    let r = app
        .post(
            &s.url("purchase-history/imports"),
            &s.w.admin_a,
            json!({ "file_name": "history.csv",
                    "data_base64": base64::engine::general_purpose::STANDARD.encode(HISTORY) }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["rows_imported"], 3);
    let r = app
        .put(
            &s.url("buying-criteria"),
            &s.w.admin_a,
            json!({ "priorities": ["price"], "preferred_suppliers": [], "excluded_suppliers": [],
                    "required_certifications": [], "price_alert_bps": 1000, "notes": "" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);

    let after = s.op(&format!("rfqs/{rfq}/comparisons"), json!({})).await;
    // Reference only: compliance, reasons, totals and ranks are exactly as before.
    assert_eq!(decisions(&before["results"]), decisions(&after["results"]));

    // S1: bearing ¥1200 vs ¥1100 last (鈴木機材, the later purchase) = +9.09 %, under the
    // line; guide ¥45000 vs ¥40000 = +12.5 %, above it.
    let r1 = by_supplier(&after["results"], s1);
    assert_eq!(r1["price_alert_bps"], 1000);
    assert_eq!(r1["price_alerts"], 1);
    let c = &r1["price_checks"];
    assert_eq!(c.as_array().unwrap().len(), 2);
    assert_eq!(c[0]["request_item_id"], json!(items[0]));
    assert_eq!(c[0]["quoted_unit_price_minor"], 1200);
    assert_eq!(c[0]["last_unit_price_milli"], 1_100_000);
    assert_eq!(c[0]["last_currency"], "JPY");
    assert_eq!(c[0]["last_purchased_on"], "2026-03-01");
    assert_eq!(c[0]["last_supplier_name"], "鈴木機材");
    assert_eq!(c[0]["change_bps"], 909);
    assert_eq!(c[0]["above_alert"], false);
    assert_eq!(c[1]["request_item_id"], json!(items[1]));
    assert_eq!(c[1]["last_supplier_name"], "山田商事");
    assert_eq!(c[1]["change_bps"], 1250);
    assert_eq!(c[1]["above_alert"], true);

    // S2: +18.18 % (above) and −5 % (cheaper, never flagged).
    let r2 = by_supplier(&after["results"], s2);
    assert_eq!(r2["price_alerts"], 1);
    assert_eq!(r2["price_checks"][0]["change_bps"], 1818);
    assert_eq!(r2["price_checks"][0]["above_alert"], true);
    assert_eq!(r2["price_checks"][1]["change_bps"], -500);
    assert_eq!(r2["price_checks"][1]["above_alert"], false);

    // S3 quoted in USD: no implicit FX, so no change and no flag, but the last price is shown.
    let r3 = by_supplier(&after["results"], s3);
    assert_eq!(r3["price_alerts"], 0);
    assert_eq!(r3["price_checks"].as_array().unwrap().len(), 2);
    for c in r3["price_checks"].as_array().unwrap() {
        assert_eq!(c["change_bps"], Value::Null);
        assert_eq!(c["above_alert"], false);
        assert_eq!(c["last_currency"], "JPY");
    }

    // The stored run returns the same to the customer's reviewer (who may read history).
    let cmp = uuid(&after["id"]);
    let r = app
        .get(&s.url(&format!("comparisons/{cmp}")), &s.manager)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["results"], after["results"]);
    // Requesters see neither the comparison nor past prices.
    let r = app
        .get(&s.url(&format!("comparisons/{cmp}")), &s.requester)
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn no_alert_line_means_no_flags(pool: PgPool) {
    let s = Setup::new(pool).await;
    let app = &s.w.app;
    let r = app
        .post(
            &s.url("purchase-history/imports"),
            &s.w.admin_a,
            json!({ "file_name": "history.csv",
                    "data_base64": base64::engine::general_purpose::STANDARD.encode(HISTORY) }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let (rfq, items) = s.rfq().await;
    s.quote(rfq, s.suppliers[0], "JPY", &items, [5000, 90_000])
        .await;
    let run = s.op(&format!("rfqs/{rfq}/comparisons"), json!({})).await;
    let r = by_supplier(&run["results"], s.suppliers[0]);
    assert_eq!(r["price_alert_bps"], Value::Null);
    assert_eq!(r["price_alerts"], 0);
    assert_eq!(r["price_checks"][0]["change_bps"], 35_455);
    assert_eq!(r["price_checks"][1]["change_bps"], 12_500);
    assert_eq!(r["price_checks"][0]["above_alert"], false);
    assert_eq!(r["price_checks"][1]["above_alert"], false);
}
