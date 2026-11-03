//! CSV files for the factory (CEO specification 2026-11-03: at first we exchange CSV files
//! instead of connecting to the factory's ERP). Orders, for its own systems, and the savings
//! ledger behind our monthly invoice. UTF-8 with a byte-order mark so Excel opens Japanese
//! text correctly; text cells that a spreadsheet would run as a formula are quoted with `'`.

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::IntoResponse;
use chrono::{Datelike, NaiveDate};
use serde::Deserialize;
use uuid::Uuid;

use crate::AppState;
use crate::api::common;
use crate::db;
use crate::error::{ApiError, ApiPath, ApiQuery, ApiResult, ErrorBody};
use crate::principal::Principal;

/// Rows in one file.
const MAX_ROWS: i64 = 50_000;

#[derive(Deserialize, utoipa::IntoParams)]
pub struct MonthQuery {
    /// `YYYY-MM`: orders placed in that month (default: all).
    pub month: Option<String>,
}

/// `2026-11` → (2026-11-01, 2026-11-30).
pub fn month_range(raw: &str) -> Option<(NaiveDate, NaiveDate)> {
    let (y, m) = raw.trim().split_once('-')?;
    if y.len() != 4 || m.len() != 2 {
        return None;
    }
    let (y, m): (i32, u32) = (y.parse().ok()?, m.parse().ok()?);
    if !(2000..=2100).contains(&y) {
        return None;
    }
    let first = NaiveDate::from_ymd_opt(y, m, 1)?;
    let next = if m == 12 {
        NaiveDate::from_ymd_opt(y + 1, 1, 1)?
    } else {
        NaiveDate::from_ymd_opt(y, m + 1, 1)?
    };
    Some((first, next.pred_opt()?))
}

fn range(q: &MonthQuery) -> ApiResult<(NaiveDate, NaiveDate)> {
    match q.month.as_deref() {
        None | Some("") => Ok((
            NaiveDate::from_ymd_opt(2000, 1, 1).ok_or(ApiError::Validation("month"))?,
            NaiveDate::from_ymd_opt(2100, 12, 31).ok_or(ApiError::Validation("month"))?,
        )),
        Some(m) => month_range(m).ok_or(ApiError::Validation("month")),
    }
}

/// One CSV cell. Text that starts like a formula is prefixed with `'`.
pub fn cell(raw: &str) -> String {
    let s = if raw.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{raw}")
    } else {
        raw.to_string()
    };
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

/// `value` × 10^-`scale` as plain decimal text, without trailing zeros: (150050, 2) → "1500.5".
pub fn decimal(value: i64, scale: u32) -> String {
    let neg = value < 0;
    let v = value.unsigned_abs();
    let base = 10u64.pow(scale);
    let (whole, frac) = (v / base, v % base);
    let mut out = if neg { "-".to_string() } else { String::new() };
    out.push_str(&whole.to_string());
    if frac > 0 {
        let f = format!("{frac:0width$}", width = scale as usize);
        out.push('.');
        out.push_str(f.trim_end_matches('0'));
    }
    out
}

fn minor_digits(code: &str) -> ApiResult<u32> {
    Ok(common::currency(code)?.minor_digits())
}

fn csv_response(
    file_name: &str,
    header_row: &[&str],
    rows: Vec<Vec<String>>,
) -> ApiResult<(HeaderMap, String)> {
    let mut out = String::from("\u{feff}");
    out.push_str(&header_row.join(","));
    out.push_str("\r\n");
    for r in rows {
        out.push_str(&r.join(","));
        out.push_str("\r\n");
    }
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{file_name}\""))
            .map_err(ApiError::internal)?,
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok((headers, out))
}

fn file_name(kind: &str, q: &MonthQuery) -> String {
    match q.month.as_deref().and_then(month_range) {
        Some((first, _)) => format!("{kind}-{:04}-{:02}.csv", first.year(), first.month()),
        None => format!("{kind}.csv"),
    }
}

#[derive(sqlx::FromRow)]
struct OrderLineRow {
    po_number: String,
    request_number: String,
    ordered_on: NaiveDate,
    supplier_name: Option<String>,
    line_no: i32,
    description: String,
    manufacturer: String,
    mpn: String,
    quantity: i32,
    unit: String,
    unit_price_minor: i64,
    currency: String,
    shipping_minor: i64,
    total_minor: i64,
    promised_on: Option<NaiveDate>,
    delivery_confirmed_on: Option<NaiveDate>,
    received: i64,
    status: String,
    order_email: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/exports/orders.csv",
    tag = "orders",
    params(("org_id" = Uuid, Path), MonthQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Order lines as CSV (text/csv, UTF-8 with BOM)"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation", body = ErrorBody),
        (status = 422, description = "Invalid month", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn orders_csv(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiQuery(q): ApiQuery<MonthQuery>,
) -> ApiResult<impl IntoResponse> {
    let view = crate::api::orders::order_reader(&p, &state, org_id).await?;
    let (from, to) = range(&q)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let lines = sqlx::query_as::<_, OrderLineRow>(
        "SELECT o.po_number, r.number AS request_number,
                (o.ordered_at AT TIME ZONE g.timezone)::date AS ordered_on,
                s.name AS supplier_name, ri.line_no, ri.description,
                coalesce(ri.verified_manufacturer, ri.manufacturer) AS manufacturer,
                CASE WHEN btrim(oi.offered_mpn) <> '' THEN oi.offered_mpn
                     ELSE coalesce(ri.verified_mpn, ri.mpn) END AS mpn,
                oi.quantity, ri.unit, oi.unit_price_minor, o.currency, o.shipping_minor,
                o.total_minor, o.promised_on, o.delivery_confirmed_on,
                coalesce((SELECT sum(d.quantity) FROM delivery_lines d
                          JOIN deliveries dv ON dv.id = d.delivery_id
                          WHERE d.order_item_id = oi.id AND dv.reversed_at IS NULL), 0)::bigint
                    AS received,
                r.status,
                (SELECT x.status FROM outbox x WHERE x.order_id = o.id AND x.kind = 'order')
                    AS order_email
         FROM purchase_orders o
         JOIN organisations g ON g.id = o.org_id
         JOIN procurement_requests r ON r.id = o.request_id
         JOIN order_items oi ON oi.order_id = o.id
         JOIN request_items ri ON ri.id = oi.request_item_id
         LEFT JOIN suppliers s ON s.id = o.supplier_id
         WHERE (o.ordered_at AT TIME ZONE g.timezone)::date BETWEEN $1 AND $2
           AND ($3::uuid IS NULL OR r.requested_by = $3)
         ORDER BY o.ordered_at, o.po_number, ri.line_no
         LIMIT $4",
    )
    .bind(from)
    .bind(to)
    .bind(view.owner_filter())
    .bind(MAX_ROWS)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let mut rows = Vec::with_capacity(lines.len());
    for l in lines {
        let d = minor_digits(&l.currency)?;
        let date = |x: Option<NaiveDate>| x.map(|d| d.to_string()).unwrap_or_default();
        rows.push(vec![
            cell(&l.po_number),
            cell(&l.request_number),
            l.ordered_on.to_string(),
            cell(l.supplier_name.as_deref().unwrap_or("")),
            l.line_no.to_string(),
            cell(&l.description),
            cell(&l.manufacturer),
            cell(&l.mpn),
            l.quantity.to_string(),
            cell(&l.unit),
            decimal(l.unit_price_minor, d),
            decimal(l.unit_price_minor.saturating_mul(i64::from(l.quantity)), d),
            l.currency,
            decimal(l.shipping_minor, d),
            decimal(l.total_minor, d),
            date(l.promised_on),
            date(l.delivery_confirmed_on),
            l.received.to_string(),
            l.status,
            l.order_email.unwrap_or_else(|| "draft".into()),
        ]);
    }
    csv_response(
        &file_name("orders", &q),
        &[
            "po_number",
            "request_number",
            "ordered_on",
            "supplier",
            "line_no",
            "description",
            "manufacturer",
            "mpn",
            "quantity",
            "unit",
            "unit_price",
            "line_total",
            "currency",
            "order_shipping",
            "order_total_before_tax",
            "promised_on",
            "confirmed_on",
            "received_quantity",
            "request_status",
            "order_email",
        ],
        rows,
    )
}

#[derive(sqlx::FromRow)]
struct LedgerRow {
    ordered_on: NaiveDate,
    po_number: String,
    request_number: String,
    line_no: i32,
    description: String,
    item_key: Option<String>,
    supplier_name: Option<String>,
    quantity: i32,
    unit_price_minor: i64,
    baseline_unit_price_milli: Option<i64>,
    baseline_source: Option<String>,
    baseline_on: Option<NaiveDate>,
    baseline_supplier: Option<String>,
    same_supplier: Option<bool>,
    currency: String,
    share_bps: Option<i32>,
    saving_minor: i64,
    fee_minor: i64,
    status: String,
    invoice_number: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/exports/savings.csv",
    tag = "savings",
    params(("org_id" = Uuid, Path), MonthQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Savings ledger lines as CSV (text/csv, UTF-8 with BOM)"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation", body = ErrorBody),
        (status = 422, description = "Invalid month", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn savings_csv(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiQuery(q): ApiQuery<MonthQuery>,
) -> ApiResult<impl IntoResponse> {
    crate::api::savings::require_read(&state, &p, org_id).await?;
    let (from, to) = range(&q)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    // `same_supplier`: the usual supplier lowered its price because we came in; that drop
    // counts as a saving like any other (CEO, 2026-11-03).
    let lines = sqlx::query_as::<_, LedgerRow>(
        "SELECT e.ordered_on, o.po_number, r.number AS request_number, ri.line_no,
                ri.description, e.item_key, s.name AS supplier_name, e.quantity,
                e.unit_price_minor, e.baseline_unit_price_milli, b.source AS baseline_source,
                b.purchased_on AS baseline_on, b.supplier_name AS baseline_supplier,
                CASE WHEN b.id IS NULL OR btrim(b.supplier_name) = '' OR s.name IS NULL THEN NULL
                     ELSE lower(btrim(b.supplier_name)) = lower(btrim(s.name)) END AS same_supplier,
                e.currency, e.share_bps, e.saving_minor, e.fee_minor, e.status,
                CASE WHEN inv.status <> 'draft' THEN inv.number END AS invoice_number
         FROM savings_entries e
         JOIN purchase_orders o ON o.id = e.order_id
         JOIN procurement_requests r ON r.id = o.request_id
         JOIN order_items oi ON oi.id = e.order_item_id
         JOIN request_items ri ON ri.id = oi.request_item_id
         LEFT JOIN suppliers s ON s.id = o.supplier_id
         LEFT JOIN savings_baselines b ON b.id = e.baseline_id
         LEFT JOIN issued_invoices inv ON inv.id = e.invoice_id
         WHERE e.ordered_on BETWEEN $1 AND $2
         ORDER BY e.ordered_on, o.po_number, ri.line_no
         LIMIT $3",
    )
    .bind(from)
    .bind(to)
    .bind(MAX_ROWS)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let mut rows = Vec::with_capacity(lines.len());
    for l in lines {
        let d = minor_digits(&l.currency)?;
        rows.push(vec![
            l.ordered_on.to_string(),
            cell(&l.po_number),
            cell(&l.request_number),
            l.line_no.to_string(),
            cell(&l.description),
            cell(l.item_key.as_deref().unwrap_or("")),
            cell(l.supplier_name.as_deref().unwrap_or("")),
            l.quantity.to_string(),
            decimal(l.unit_price_minor, d),
            l.baseline_unit_price_milli
                .map(|m| decimal(m, d + 3))
                .unwrap_or_default(),
            l.baseline_source.unwrap_or_default(),
            l.baseline_on.map(|d| d.to_string()).unwrap_or_default(),
            cell(l.baseline_supplier.as_deref().unwrap_or("")),
            match l.same_supplier {
                Some(true) => "yes".into(),
                Some(false) => "no".into(),
                None => String::new(),
            },
            l.currency,
            l.share_bps
                .map(|b| decimal(i64::from(b), 2))
                .unwrap_or_default(),
            decimal(l.saving_minor, d),
            decimal(l.fee_minor, d),
            l.status,
            cell(l.invoice_number.as_deref().unwrap_or("")),
        ]);
    }
    csv_response(
        &file_name("savings", &q),
        &[
            "ordered_on",
            "po_number",
            "request_number",
            "line_no",
            "description",
            "item_key",
            "supplier",
            "quantity",
            "unit_price",
            "baseline_unit_price",
            "baseline_source",
            "baseline_date",
            "baseline_supplier",
            "same_supplier_as_before",
            "currency",
            "share_percent",
            "saving",
            "fee",
            "status",
            "invoice_number",
        ],
        rows,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months() {
        let d = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        assert_eq!(
            month_range("2026-11"),
            Some((d(2026, 11, 1), d(2026, 11, 30)))
        );
        assert_eq!(
            month_range("2026-12"),
            Some((d(2026, 12, 1), d(2026, 12, 31)))
        );
        assert_eq!(
            month_range("2028-02"),
            Some((d(2028, 2, 1), d(2028, 2, 29)))
        );
        for bad in ["2026-13", "2026-1", "26-11", "x", "1999-01"] {
            assert_eq!(month_range(bad), None, "{bad}");
        }
    }

    #[test]
    fn cells_and_numbers() {
        assert_eq!(cell("ボルト, M8"), "\"ボルト, M8\"");
        assert_eq!(cell("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(cell("=HYPERLINK(1)"), "'=HYPERLINK(1)");
        assert_eq!(cell("-1+2"), "'-1+2");
        assert_eq!(decimal(1_500_000, 3), "1500");
        assert_eq!(decimal(100_500, 3), "100.5");
        assert_eq!(decimal(150_050, 2), "1500.5");
        assert_eq!(decimal(5000, 2), "50");
        assert_eq!(decimal(0, 0), "0");
    }
}
