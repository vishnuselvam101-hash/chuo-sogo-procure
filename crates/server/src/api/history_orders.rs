//! Orders completed in this system feed the customer's purchase history: when an order
//! reconciles, its lines are recorded as one history import (source `order`) at the agreed
//! PO price, so the price reference grows as the service is used. Withdrawn like any import.

use chrono::NaiveDate;
use procure_domain::history;
use serde_json::json;
use uuid::Uuid;

use crate::api::common::Audit;
use crate::db;
use crate::error::{ApiError, ApiResult};

/// `history_imports.file_name` limit.
const FILE_NAME_CHARS: usize = 255;
/// `purchase_history.lead_time_days` range.
const MAX_LEAD_TIME_DAYS: i32 = 3650;

#[derive(sqlx::FromRow)]
struct Line {
    line_no: i32,
    purchased_on: NaiveDate,
    supplier_name: String,
    description: String,
    manufacturer: String,
    mpn: String,
    quantity: i32,
    unit: String,
    unit_price_minor: i64,
    currency: String,
    lead_time_days: Option<i32>,
}

/// Records a reconciled order into the organisation's purchase history, in the caller's
/// transaction. Does nothing when the order is already recorded.
pub async fn record_order(
    tx: &mut db::Tx,
    audit: &Audit<'_>,
    org_id: Uuid,
    order_id: Uuid,
    user: Uuid,
) -> ApiResult<()> {
    let recorded: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM history_imports WHERE order_id = $1)")
            .bind(order_id)
            .fetch_one(&mut **tx)
            .await?;
    if recorded {
        return Ok(());
    }
    let po_number: String =
        sqlx::query_scalar("SELECT po_number FROM purchase_orders WHERE id = $1")
            .bind(order_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    // Dates are the organisation's calendar days. The supplier directory is RLS-limited for
    // customers, as in the order view: an unseen supplier is recorded without a name.
    let lines = sqlx::query_as::<_, Line>(
        "SELECT ri.line_no,
                (o.ordered_at AT TIME ZONE g.timezone)::date AS purchased_on,
                coalesce(s.name, '') AS supplier_name,
                ri.description,
                CASE WHEN oi.alternative THEN ''
                     ELSE coalesce(ri.verified_manufacturer, ri.manufacturer) END AS manufacturer,
                CASE WHEN btrim(oi.offered_mpn) <> '' THEN oi.offered_mpn
                     ELSE coalesce(ri.verified_mpn, ri.mpn) END AS mpn,
                oi.quantity,
                ri.unit,
                oi.unit_price_minor,
                o.currency,
                (SELECT (max(dv.received_at) AT TIME ZONE g.timezone)::date
                        - (o.ordered_at AT TIME ZONE g.timezone)::date
                 FROM delivery_lines dl JOIN deliveries dv ON dv.id = dl.delivery_id
                 WHERE dl.order_item_id = oi.id AND dv.reversed_at IS NULL) AS lead_time_days
         FROM order_items oi
         JOIN purchase_orders o ON o.id = oi.order_id
         JOIN organisations g ON g.id = o.org_id
         JOIN request_items ri ON ri.id = oi.request_item_id
         LEFT JOIN suppliers s ON s.id = o.supplier_id
         WHERE oi.order_id = $1
         ORDER BY ri.line_no",
    )
    .bind(order_id)
    .fetch_all(&mut **tx)
    .await?;

    let mut ids = Vec::with_capacity(lines.len());
    let mut source_rows = Vec::with_capacity(lines.len());
    let mut dates = Vec::with_capacity(lines.len());
    let mut suppliers = Vec::with_capacity(lines.len());
    let mut descriptions = Vec::with_capacity(lines.len());
    let mut makers = Vec::with_capacity(lines.len());
    let mut mpns = Vec::with_capacity(lines.len());
    let mut keys = Vec::with_capacity(lines.len());
    let mut quantities = Vec::with_capacity(lines.len());
    let mut units = Vec::with_capacity(lines.len());
    let mut prices = Vec::with_capacity(lines.len());
    let mut currencies = Vec::with_capacity(lines.len());
    let mut lead_times = Vec::with_capacity(lines.len());
    let overflow = || ApiError::internal(std::io::Error::other("history amount overflow"));
    for l in lines {
        let Some(key) = history::item_key(&l.mpn, &l.description) else {
            continue;
        };
        quantities.push(
            i64::from(l.quantity)
                .checked_mul(1000)
                .ok_or_else(overflow)?,
        );
        // The PO price is the agreed price.
        prices.push(l.unit_price_minor.checked_mul(1000).ok_or_else(overflow)?);
        ids.push(Uuid::now_v7());
        source_rows.push(l.line_no);
        dates.push(l.purchased_on);
        suppliers.push(l.supplier_name);
        descriptions.push(l.description);
        makers.push(l.manufacturer);
        mpns.push(l.mpn);
        keys.push(key);
        units.push(l.unit);
        currencies.push(l.currency);
        lead_times.push(l.lead_time_days.map(|d| d.clamp(0, MAX_LEAD_TIME_DAYS)));
    }
    let rows = i32::try_from(ids.len()).map_err(ApiError::internal)?;
    let file_name: String = format!("PO {po_number}")
        .chars()
        .take(FILE_NAME_CHARS)
        .collect();
    let import_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO history_imports (id, org_id, file_name, mapping, rows_imported, rows_skipped,
                                      imported_by, source, order_id)
         VALUES ($1, $2, $3, '[]'::jsonb, $4, 0, $5, 'order', $6)",
    )
    .bind(import_id)
    .bind(org_id)
    .bind(&file_name)
    .bind(rows)
    .bind(user)
    .bind(order_id)
    .execute(&mut **tx)
    .await?;
    if !ids.is_empty() {
        sqlx::query(
            "INSERT INTO purchase_history (id, org_id, import_id, source_row, purchased_on,
                 supplier_name, description, manufacturer, mpn, item_key, quantity_milli, unit,
                 unit_price_milli, price_derived, currency, lead_time_days, notes)
             SELECT u.id, $1, $2, u.source_row, u.purchased_on, u.supplier_name, u.description,
                    u.manufacturer, u.mpn, u.item_key, u.quantity_milli, u.unit,
                    u.unit_price_milli, false, u.currency, u.lead_time_days, ''
             FROM UNNEST($3::uuid[], $4::int[], $5::date[], $6::text[], $7::text[], $8::text[],
                         $9::text[], $10::text[], $11::bigint[], $12::text[], $13::bigint[],
                         $14::text[], $15::int[])
                  AS u(id, source_row, purchased_on, supplier_name, description, manufacturer,
                       mpn, item_key, quantity_milli, unit, unit_price_milli, currency,
                       lead_time_days)",
        )
        .bind(org_id)
        .bind(import_id)
        .bind(&ids)
        .bind(&source_rows)
        .bind(&dates)
        .bind(&suppliers)
        .bind(&descriptions)
        .bind(&makers)
        .bind(&mpns)
        .bind(&keys)
        .bind(&quantities)
        .bind(&units)
        .bind(&prices)
        .bind(&currencies)
        .bind(&lead_times)
        .execute(&mut **tx)
        .await?;
    }
    audit
        .record(
            tx,
            "purchase_history.recorded_from_order",
            "history_import",
            import_id,
            json!({ "order": order_id, "po_number": po_number, "rows_imported": rows }),
        )
        .await
}
