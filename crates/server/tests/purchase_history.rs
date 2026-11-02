//! Purchase history import, price reference and buying criteria, against a real database.

#![allow(clippy::unwrap_used)]

mod common;

use axum::http::StatusCode;
use base64::Engine;
use common::*;
use serde_json::{Value, json};
use sqlx::PgPool;

/// A typical Japanese ERP export: title block, Shift_JIS, full-width part numbers, a row the
/// importer must refuse, and a blank line.
const EXPORT: &str = "購買実績一覧\r\n\
出力日,2026/10/01\r\n\
伝票日付,仕入先名,品名,型番,数量,単価（円）,金額(税抜),備考\r\n\
2025/04/10,山田商事,深溝玉軸受,６２０４ＺＺ,100,320,,\r\n\
令和7年9月1日,鈴木機材,深溝玉軸受,6204-ZZ,50,,15500,急ぎ\r\n\
2026/02/03,山田商事,\"六角ボルト, M8\",,\"1,000\",12.5,,\r\n\
先月,山田商事,ナット,,10,5,,\r\n\
,,,,,,,\r\n";

fn body(file: &str, bytes: &[u8]) -> Value {
    json!({
        "file_name": file,
        "data_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

fn sjis(text: &str) -> Vec<u8> {
    encoding_rs::SHIFT_JIS.encode(text).0.into_owned()
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn import_preview_reference_and_withdraw(pool: PgPool) {
    let w = world(pool).await;
    let app = &w.app;
    let url = |p: &str| format!("/api/v1/orgs/{}/{p}", w.org_a);
    let file = body("購買実績.csv", &sjis(EXPORT));

    // Preview stores nothing and explains every skipped row.
    let r = app
        .post(&url("purchase-history/preview"), &w.admin_a, file.clone())
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["header_row"], 2);
    assert_eq!(r.body["data_rows"], 4);
    assert_eq!(r.body["valid_rows"], 3);
    assert_eq!(
        r.body["errors"],
        json!([{ "row": 7, "code": "date_invalid" }])
    );
    assert_eq!(r.body["sample"][0]["mpn"], "6204ZZ");
    // 15 500 yen for 50 = 310 yen each, worked out from the total.
    assert_eq!(r.body["sample"][1]["purchased_on"], "2025-09-01");
    assert_eq!(r.body["sample"][1]["unit_price_milli"], 310_000);
    assert_eq!(r.body["sample"][1]["price_derived"], true);
    assert_eq!(r.body["sample"][2]["description"], "六角ボルト, M8");
    assert_eq!(r.body["sample"][2]["quantity_milli"], 1_000_000);
    assert_eq!(r.body["sample"][2]["unit_price_milli"], 12_500);
    let r = app.get(&url("purchase-history"), &w.admin_a).await;
    assert_eq!(r.body, json!([]));

    let r = app
        .post(&url("purchase-history/imports"), &w.admin_a, file)
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(
        (
            r.body["rows_imported"].as_i64(),
            r.body["rows_skipped"].as_i64()
        ),
        (Some(3), Some(1))
    );
    let import = uuid(&r.body["id"]);

    // Search: every word must match; LIKE wildcards are literal.
    let r = app
        .get(&url("purchase-history?q=山田%20軸受"), &w.admin_a)
        .await;
    assert_eq!(r.body.as_array().unwrap().len(), 1, "{:?}", r.body);
    let r = app.get(&url("purchase-history?q=%25"), &w.admin_a).await;
    assert_eq!(r.body, json!([]));

    // A request for the bearing (part number typed differently) and an unknown part.
    let r = app
        .post(
            &url("requests"),
            &w.admin_a,
            json!({
                "title": "軸受補充",
                "currency": "JPY",
                "items": [
                    { "description": "Ball bearing", "mpn": "6204 zz", "quantity": 20 },
                    { "description": "六角ボルト,  M8", "quantity": 500 },
                    { "description": "Never bought", "mpn": "XYZ-1", "quantity": 1 }
                ]
            }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let request = uuid(&r.body["id"]);
    let r = app
        .get(
            &url(&format!("requests/{request}/price-reference")),
            &w.admin_a,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let bearing = &r.body[0];
    assert_eq!(bearing["matched_on"], "mpn");
    assert_eq!(bearing["summary"]["purchases"], 2);
    assert_eq!(bearing["summary"]["last"]["supplier_name"], "鈴木機材");
    assert_eq!(bearing["summary"]["last"]["unit_price_milli"], 310_000);
    assert_eq!(bearing["summary"]["min_price_milli"], 310_000);
    assert_eq!(bearing["summary"]["max_price_milli"], 320_000);
    assert_eq!(bearing["summary"]["suppliers"], 2);
    assert_eq!(bearing["recent"].as_array().unwrap().len(), 2);
    // Description match ignores the double space; "六角ボルト, M8" was typed with one.
    assert_eq!(r.body[1]["matched_on"], "description");
    assert_eq!(r.body[1]["summary"]["last"]["unit_price_milli"], 12_500);
    assert_eq!(r.body[2]["summary"], Value::Null);

    // Withdrawing removes the rows, keeps the import listed, and cannot happen twice.
    let withdraw = url(&format!("purchase-history/imports/{import}/withdraw"));
    let r = app.post(&withdraw, &w.admin_a, json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert!(r.body["withdrawn_at"].is_string());
    let r = app.post(&withdraw, &w.admin_a, json!({})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let r = app.get(&url("purchase-history"), &w.admin_a).await;
    assert_eq!(r.body, json!([]));
    let r = app.get(&url("purchase-history/imports"), &w.admin_a).await;
    assert_eq!(r.body.as_array().unwrap().len(), 1);

    let r = app.get(&url("audit?limit=100"), &w.admin_a).await;
    let actions: Vec<&str> = r.body["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert!(actions.contains(&"purchase_history.imported"));
    assert!(actions.contains(&"purchase_history.withdrawn"));
    assert_eq!(r.body["chain_valid"], true);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn mapping_can_be_given_and_bad_files_are_refused(pool: PgPool) {
    let w = world(pool).await;
    let app = &w.app;
    let url = format!("/api/v1/orgs/{}/purchase-history/preview", w.org_a);
    // Headers nobody could guess: the person maps the columns.
    let csv = "A,B,C\n2026-01-05,Hex nut,3\n";
    let r = app
        .post(&url, &w.admin_a, body("x.csv", csv.as_bytes()))
        .await;
    assert_eq!(r.code(), "validation_failed");
    assert_eq!(r.body["error"]["detail"], "header_not_found");
    let mut b = body("x.csv", csv.as_bytes());
    b["header_row"] = json!(0);
    b["mapping"] = json!(["date", "description", "unit_price"]);
    b["currency"] = json!("USD");
    let r = app.post(&url, &w.admin_a, b.clone()).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["sample"][0]["currency"], "USD");
    assert_eq!(r.body["sample"][0]["unit_price_milli"], 300_000);

    for (mapping, detail) in [
        (json!(["date", "description", "date"]), "mapping_duplicate"),
        (json!(["date", "description", null]), "mapping_incomplete"),
        (json!(["date", "nonsense", "unit_price"]), "mapping"),
    ] {
        let mut bad = b.clone();
        bad["mapping"] = mapping;
        let r = app.post(&url, &w.admin_a, bad).await;
        assert_eq!(r.body["error"]["detail"], detail);
    }
    let r = app
        .post(&url, &w.admin_a, body("x.pdf", b"%PDF-1.4 x"))
        .await;
    assert_eq!(r.body["error"]["detail"], "file_type");
    let r = app.post(&url, &w.admin_a, body("x.csv", b",,\n,,\n")).await;
    assert_eq!(r.body["error"]["detail"], "nothing_readable");
    // Nothing readable at all: no empty import is stored.
    let only_bad = "日付,品名,単価\n先月,ナット,5\n";
    let r = app
        .post(
            &format!("/api/v1/orgs/{}/purchase-history/imports", w.org_a),
            &w.admin_a,
            body("x.csv", only_bad.as_bytes()),
        )
        .await;
    assert_eq!(r.body["error"]["detail"], "nothing_readable");
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn who_may_see_and_change_history(pool: PgPool) {
    let w = world(pool).await;
    let app = &w.app;
    let url = |org, p: &str| format!("/api/v1/orgs/{org}/{p}");
    let r = app
        .post(
            &url(w.org_a, "purchase-history/imports"),
            &w.admin_a,
            body(
                "h.csv",
                "日付,品名,単価\n2026/1/5,秘密の部品,99\n".as_bytes(),
            ),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);

    // Another customer cannot see or touch it; the tables are tenant-isolated.
    let r = app.get(&url(w.org_a, "purchase-history"), &w.admin_b).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = app.get(&url(w.org_b, "purchase-history"), &w.admin_b).await;
    assert_eq!(r.body, json!([]));

    // Requesters do not see past prices; finance reads but cannot import.
    let requester = w
        .member(w.org_a, &w.admin_a, "req@a.test", "requester")
        .await;
    let finance = w.member(w.org_a, &w.admin_a, "fin@a.test", "finance").await;
    let r = app.get(&url(w.org_a, "purchase-history"), &requester).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = app.get(&url(w.org_a, "purchase-history"), &finance).await;
    assert_eq!(r.body.as_array().unwrap().len(), 1);
    let r = app
        .post(
            &url(w.org_a, "purchase-history/preview"),
            &finance,
            body("h.csv", b"x"),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // Chuo-Sogo staff: nothing without an assignment; with one, they can onboard the data.
    let operator = w
        .member(
            w.platform_org,
            &w.super_admin,
            "op@chuo-sogo.test",
            "procurement_operator",
        )
        .await;
    let r = app.get(&url(w.org_a, "purchase-history"), &operator).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let op_id = w.user_id(&operator).await;
    let r = app
        .post(
            &url(w.org_a, "assignments"),
            &w.super_admin,
            json!({ "user_id": op_id }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let r = app
        .post(
            &url(w.org_a, "purchase-history/imports"),
            &operator,
            body("h2.csv", "日付,品名,単価\n2026/2/5,部品B,10\n".as_bytes()),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
}

#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn buying_criteria(pool: PgPool) {
    let w = world(pool).await;
    let app = &w.app;
    let url = format!("/api/v1/orgs/{}/buying-criteria", w.org_a);
    let r = app.get(&url, &w.admin_a).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["priorities"], json!([]));
    assert_eq!(r.body["updated_at"], Value::Null);

    let criteria = json!({
        "priorities": ["lead_time", "price", "existing_supplier"],
        "preferred_suppliers": ["山田商事", " 山田商事 "],
        "excluded_suppliers": ["Cheap Parts Ltd"],
        "required_certifications": ["ISO 9001"],
        "max_lead_time_days": 14,
        "price_alert_bps": 1000,
        "notes": "ライン停止時は納期優先。\n海外品は承認者に相談。"
    });
    let r = app.put(&url, &w.admin_a, criteria.clone()).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["preferred_suppliers"], json!(["山田商事"]));
    assert_eq!(r.body["price_alert_bps"], 1000);
    assert!(r.body["updated_at"].is_string());

    for (field, value, detail) in [
        ("priorities", json!(["price", "price"]), "priorities"),
        ("priorities", json!(["cheapest"]), "priorities"),
        (
            "excluded_suppliers",
            json!(["山田商事"]),
            "supplier_both_lists",
        ),
        ("max_lead_time_days", json!(0), "max_lead_time_days"),
    ] {
        let mut bad = criteria.clone();
        bad[field] = value;
        let r = app.put(&url, &w.admin_a, bad).await;
        assert_eq!(r.body["error"]["detail"], detail, "{field}");
    }
    let requester = w
        .member(w.org_a, &w.admin_a, "req@a.test", "requester")
        .await;
    let r = app.put(&url, &requester, criteria).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = app
        .get(
            &format!("/api/v1/orgs/{}/audit?limit=50", w.org_a),
            &w.admin_a,
        )
        .await;
    assert!(
        r.body["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "buying_criteria.updated")
    );
}

/// A real Excel workbook: a cover sheet without a table, dates stored as Excel dates, a sub-yen
/// price stored as a float.
#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn excel_workbooks_are_read(pool: PgPool) {
    let w = world(pool).await;
    let xlsx = include_bytes!("fixtures/purchase-history.xlsx");
    let r = w
        .app
        .post(
            &format!("/api/v1/orgs/{}/purchase-history/preview", w.org_a),
            &w.admin_a,
            body("購買実績.xlsx", xlsx),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["header_row"], 2);
    assert_eq!(r.body["valid_rows"], 2);
    assert_eq!(
        r.body["errors"],
        json!([{ "row": 6, "code": "date_invalid" }])
    );
    let first = &r.body["sample"][0];
    assert_eq!(first["purchased_on"], "2025-04-10");
    assert_eq!(first["manufacturer"], "NSK");
    assert_eq!(first["unit_price_milli"], 320_000);
    assert_eq!(first["price_derived"], false);
    let resistor = &r.body["sample"][1];
    assert_eq!(resistor["purchased_on"], "2025-09-01");
    assert_eq!(resistor["description"], "抵抗器 1kΩ");
    assert_eq!(resistor["quantity_milli"], 5_000_000);
    assert_eq!(resistor["unit_price_milli"], 850);
}

/// A scanned paper record (and the same page as a FAX TIFF and an image-only PDF) is read by
/// OCR on this server, previewed with the OCR flag, and stored as an `ocr` import.
#[sqlx::test(migrator = "procure_server::MIGRATOR")]
async fn scanned_records_are_read_by_ocr(pool: PgPool) {
    let w = world(pool).await;
    let url = |p: &str| format!("/api/v1/orgs/{}/{p}", w.org_a);
    let png = include_bytes!("fixtures/purchase-scan.png");
    let r = w
        .app
        .post(
            &url("purchase-history/preview"),
            &w.admin_a,
            body("購買記録.png", png),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["ocr"], true);
    assert_eq!(r.body["header_row"], 1);
    assert_eq!(r.body["valid_rows"], 3, "{:?}", r.body);
    let first = &r.body["sample"][0];
    assert_eq!(first["purchased_on"], "2024-01-15");
    assert_eq!(first["supplier_name"], "山田商事");
    assert_eq!(first["mpn"], "M8-30");
    assert_eq!(first["quantity_milli"], 100_000);
    assert_eq!(first["unit_price_milli"], 12_500);
    assert_eq!(r.body["sample"][2]["unit_price_milli"], 3_200);

    let r = w
        .app
        .post(
            &url("purchase-history/imports"),
            &w.admin_a,
            body("購買記録.png", png),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["source"], "ocr");
    assert_eq!(r.body["rows_imported"], 3);
    let r = w.app.get(&url("purchase-history?q=6204"), &w.admin_a).await;
    assert_eq!(r.body[0]["unit_price_milli"], 480_000);

    // The same page as an image-only PDF goes through OCR too.
    let pdf = include_bytes!("fixtures/purchase-scan.pdf");
    let r = w
        .app
        .post(
            &url("purchase-history/preview"),
            &w.admin_a,
            body("scan.pdf", pdf),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["ocr"], true);
    assert_eq!(r.body["valid_rows"], 3);

    // A spreadsheet is not OCR.
    let r = w
        .app
        .post(
            &url("purchase-history/preview"),
            &w.admin_a,
            body(
                "h.csv",
                "日付,品名,数量,単価\n2024/1/5,ボルト,1,10\n".as_bytes(),
            ),
        )
        .await;
    assert_eq!(r.body["ocr"], false);
}
