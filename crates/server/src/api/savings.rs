//! Savings share: how Chuo-Sogo is paid (procure_domain::savings, migration 0016). When an
//! order reconciles, each line is compared with the price the factory paid before we bought
//! for it; the agreed share of the saving becomes a ledger row, and the admin console bills
//! the open rows on a Chuo-Sogo invoice. No saving, no fee.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use procure_domain::history::{self, Day};
use procure_domain::money::{Currency, Money};
use procure_domain::rbac::{self, Access, Permission};
use procure_domain::savings::{self, Baseline, OrderLine, SavingsError};
use procure_domain::workflow::Side;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::api::admin::AdminPrincipal;
use crate::api::billing::{self, CreatedInvoice, InvoiceInput, LineInput};
use crate::api::common::{self, Audit};
use crate::api::history::key_groups;
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiPath, ApiResult, ErrorBody};
use crate::principal::Principal;

/// Lines on one Chuo-Sogo invoice (`issued_invoice_lines.position` limit): one per order.
const MAX_ORDERS_PER_INVOICE: usize = 200;
/// Standard consumption tax on our fee.
const FEE_TAX_BPS: u32 = 1000;

fn day(d: NaiveDate) -> ApiResult<Day> {
    Day::new(d.year(), d.month(), d.day()).ok_or(ApiError::Validation("date"))
}

fn naive(d: Day) -> ApiResult<NaiveDate> {
    NaiveDate::from_ymd_opt(d.year, d.month, d.day)
        .ok_or_else(|| ApiError::internal(std::io::Error::other("date out of range")))
}

#[derive(sqlx::FromRow)]
struct AgreementRow {
    share_bps: i32,
    starts_on: NaiveDate,
    ends_on: NaiveDate,
}

#[derive(sqlx::FromRow, Clone)]
struct BaselineRow {
    id: Uuid,
    unit_price_milli: i64,
    currency: String,
    unit: String,
    confirmed: bool,
}

const BASELINE_COLUMNS: &str =
    "id, unit_price_milli, currency, unit, confirmed_at IS NOT NULL AS confirmed";

/// The baseline for an item (any key of its alias group), fixing one from the factory's own
/// purchase records dated before the agreement started when none is fixed yet. Records of
/// orders placed through us (`source = 'order'`) never count: they are our price, not the
/// price paid before.
async fn baseline_for(
    tx: &mut Tx,
    org_id: Uuid,
    group: &[String],
    starts_on: NaiveDate,
    user: Uuid,
) -> ApiResult<Option<BaselineRow>> {
    let fixed = sqlx::query_as::<_, BaselineRow>(&format!(
        "SELECT {BASELINE_COLUMNS} FROM savings_baselines
         WHERE item_key = ANY($1) ORDER BY set_at, id LIMIT 1"
    ))
    .bind(group)
    .fetch_optional(&mut **tx)
    .await?;
    if fixed.is_some() {
        return Ok(fixed);
    }
    #[derive(sqlx::FromRow)]
    struct Past {
        id: Uuid,
        purchased_on: NaiveDate,
        supplier_name: String,
        unit_price_milli: i64,
        currency: String,
        unit: String,
        imported_by: Uuid,
        factory_import: bool,
    }
    let past = sqlx::query_as::<_, Past>(
        "SELECT h.id, h.purchased_on, h.supplier_name, h.unit_price_milli, h.currency, h.unit,
                i.imported_by,
                EXISTS (SELECT 1 FROM memberships m
                        WHERE m.user_id = i.imported_by AND m.org_id = $3
                          AND m.role IN ('customer_admin', 'procurement_manager', 'requester',
                                         'finance')) AS factory_import
         FROM purchase_history h JOIN history_imports i ON i.id = h.import_id
         WHERE h.item_key = ANY($1) AND i.source <> 'order' AND i.withdrawn_at IS NULL
           AND h.purchased_on < $2
         ORDER BY h.purchased_on DESC, h.id DESC LIMIT 1",
    )
    .bind(group)
    .bind(starts_on)
    .bind(org_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(past) = past else {
        return Ok(None);
    };
    // The factory's own record, with its unit, is the price it paid. A record our staff
    // imported for them, or one without a unit (a price per 100 would count 100 times),
    // waits for a factory approver to confirm it.
    let confirmed_by =
        (past.factory_import && !past.unit.trim().is_empty()).then_some(past.imported_by);
    sqlx::query(
        "INSERT INTO savings_baselines (id, org_id, item_key, source, history_id, purchased_on,
             supplier_name, unit_price_milli, currency, unit, set_by, confirmed_by, confirmed_at)
         VALUES ($1, $2, $3, 'history', $4, $5, $6, $7, $8, $9, $10, $11,
                 CASE WHEN $11::uuid IS NULL THEN NULL ELSE now() END)
         ON CONFLICT (org_id, item_key) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(org_id)
    .bind(&group[0])
    .bind(past.id)
    .bind(past.purchased_on)
    .bind(&past.supplier_name)
    .bind(past.unit_price_milli)
    .bind(&past.currency)
    .bind(&past.unit)
    .bind(user)
    .bind(confirmed_by)
    .execute(&mut **tx)
    .await?;
    // Another reconcile may have fixed it first; either way the stored row is the baseline.
    Ok(sqlx::query_as::<_, BaselineRow>(&format!(
        "SELECT {BASELINE_COLUMNS} FROM savings_baselines
         WHERE item_key = ANY($1) ORDER BY set_at, id LIMIT 1"
    ))
    .bind(group)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Prices one order line against a confirmed baseline: (status, saving, fee).
fn price_line(
    b: &BaselineRow,
    unit_price_minor: i64,
    quantity: i32,
    unit: &str,
    currency: Currency,
    share_bps: i32,
) -> ApiResult<(&'static str, i64, i64)> {
    let base = Baseline {
        unit_price_milli: b.unit_price_milli,
        currency: common::currency(&b.currency)?,
        unit: b.unit.clone(),
    };
    let line = OrderLine {
        unit_price: Money::from_minor(unit_price_minor, currency),
        quantity: u32::try_from(quantity).map_err(ApiError::internal)?,
        unit: unit.to_string(),
    };
    let bps = u32::try_from(share_bps).map_err(ApiError::internal)?;
    Ok(match savings::share(&base, &line, bps) {
        Ok(s) if s.fee.minor() > 0 => ("counted", s.saving.minor(), s.fee.minor()),
        Ok(s) => ("no_saving", s.saving.minor(), 0),
        Err(SavingsError::CurrencyMismatch | SavingsError::UnitMismatch) => ("mismatch", 0, 0),
        Err(e) => return Err(ApiError::internal(e)),
    })
}

/// The keys confirmed as the same item, the item's own key first.
fn ordered_group(key: &str, group: &[String]) -> Vec<String> {
    let mut g: Vec<String> = vec![key.to_string()];
    g.extend(group.iter().filter(|k| *k != key).cloned());
    g
}

/// 5000 → "50", 1250 → "12.5", 3333 → "33.33".
fn percent_text(bps: i32) -> String {
    let (whole, frac) = (bps / 100, bps % 100);
    match frac {
        0 => whole.to_string(),
        f if f % 10 == 0 => format!("{whole}.{}", f / 10),
        f => format!("{whole}.{f:02}"),
    }
}

/// "JPY 12,345" / "USD 1,234.50" for invoice line text.
fn amount_text(m: Money) -> String {
    let digits = m.currency().minor_digits();
    let scale = 10i64.pow(digits);
    let (whole, frac) = (m.minor() / scale, m.minor() % scale);
    let raw = whole.to_string();
    let mut grouped = String::new();
    for (i, c) in raw.chars().enumerate() {
        if i > 0 && (raw.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    if digits == 0 {
        format!("{} {grouped}", m.currency().code())
    } else {
        format!(
            "{} {grouped}.{:0width$}",
            m.currency().code(),
            frac,
            width = digits as usize
        )
    }
}

/// Writes the savings ledger rows of a reconciled order, in the caller's transaction. Does
/// nothing when the order is already recorded.
pub async fn record_order(
    tx: &mut Tx,
    audit: &Audit<'_>,
    org_id: Uuid,
    order_id: Uuid,
    user: Uuid,
) -> ApiResult<()> {
    let recorded: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM savings_entries WHERE order_id = $1)")
            .bind(order_id)
            .fetch_one(&mut **tx)
            .await?;
    if recorded {
        return Ok(());
    }
    let agreement = sqlx::query_as::<_, AgreementRow>(
        "SELECT share_bps, starts_on, ends_on FROM savings_agreements WHERE org_id = $1",
    )
    .bind(org_id)
    .fetch_optional(&mut **tx)
    .await?;
    #[derive(sqlx::FromRow)]
    struct Line {
        order_item_id: Uuid,
        ordered_on: NaiveDate,
        description: String,
        mpn: String,
        quantity: i32,
        unit: String,
        unit_price_minor: i64,
        currency: String,
    }
    let lines = sqlx::query_as::<_, Line>(
        "SELECT oi.id AS order_item_id,
                (o.ordered_at AT TIME ZONE g.timezone)::date AS ordered_on,
                ri.description,
                CASE WHEN btrim(oi.offered_mpn) <> '' THEN oi.offered_mpn
                     ELSE coalesce(ri.verified_mpn, ri.mpn) END AS mpn,
                oi.quantity, ri.unit, oi.unit_price_minor, o.currency
         FROM order_items oi
         JOIN purchase_orders o ON o.id = oi.order_id
         JOIN organisations g ON g.id = o.org_id
         JOIN request_items ri ON ri.id = oi.request_item_id
         WHERE oi.order_id = $1
         ORDER BY ri.line_no",
    )
    .bind(order_id)
    .fetch_all(&mut **tx)
    .await?;
    let keys: Vec<Option<String>> = lines
        .iter()
        .map(|l| history::item_key(&l.mpn, &l.description))
        .collect();
    let own: Vec<String> = keys.iter().flatten().cloned().collect();
    let groups: BTreeMap<String, Vec<String>> = key_groups(tx, &own).await?;

    let (mut counted, mut saving_total, mut fee_total) = (0usize, 0i64, 0i64);
    for (l, key) in lines.iter().zip(keys) {
        let currency = common::currency(&l.currency)?;
        let mut baseline: Option<BaselineRow> = None;
        let mut share_bps: Option<i32> = None;
        let (status, saving, fee) = match (&agreement, &key) {
            (None, _) => ("no_agreement", 0, 0),
            (Some(a), _) if !(a.starts_on <= l.ordered_on && l.ordered_on <= a.ends_on) => {
                ("outside_term", 0, 0)
            }
            (Some(_), None) => ("no_baseline", 0, 0),
            (Some(a), Some(k)) => {
                let group = ordered_group(k, groups.get(k).map_or(&[][..], |g| g.as_slice()));
                baseline = baseline_for(tx, org_id, &group, a.starts_on, user).await?;
                share_bps = Some(a.share_bps);
                match &baseline {
                    None => ("no_baseline", 0, 0),
                    Some(b) if !b.confirmed => ("unconfirmed_baseline", 0, 0),
                    Some(b) => price_line(
                        b,
                        l.unit_price_minor,
                        l.quantity,
                        &l.unit,
                        currency,
                        a.share_bps,
                    )?,
                }
            }
        };
        if status == "counted" {
            counted += 1;
            saving_total = saving_total.saturating_add(saving);
            fee_total = fee_total.saturating_add(fee);
        }
        sqlx::query(
            "INSERT INTO savings_entries (id, org_id, order_id, order_item_id, item_key,
                 baseline_id, ordered_on, quantity, unit_price_minor, baseline_unit_price_milli,
                 currency, share_bps, saving_minor, fee_minor, status)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
        )
        .bind(Uuid::now_v7())
        .bind(org_id)
        .bind(order_id)
        .bind(l.order_item_id)
        .bind(key.as_deref())
        .bind(baseline.as_ref().map(|b| b.id))
        .bind(l.ordered_on)
        .bind(l.quantity)
        .bind(l.unit_price_minor)
        .bind(baseline.as_ref().map(|b| b.unit_price_milli))
        .bind(&l.currency)
        .bind(share_bps)
        .bind(saving)
        .bind(fee)
        .bind(status)
        .execute(&mut **tx)
        .await?;
    }
    audit
        .record(
            tx,
            "savings.recorded",
            "purchase_order",
            order_id,
            json!({ "lines": lines.len(), "counted": counted,
                    "saving_minor": saving_total, "fee_minor": fee_total }),
        )
        .await
}

// ---------------------------------------------------------------------------------------
// Views shared by the factory and the admin console.

#[derive(Serialize, utoipa::ToSchema)]
pub struct AgreementView {
    pub share_bps: i32,
    pub term_months: i32,
    pub starts_on: NaiveDate,
    pub ends_on: NaiveDate,
    pub notes: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct BaselineView {
    pub id: Uuid,
    pub item_key: String,
    /// `history` (the factory's own record before the agreement) or `agreed`.
    pub source: String,
    pub purchased_on: Option<NaiveDate>,
    pub supplier_name: String,
    /// Thousandths of a minor unit.
    pub unit_price_milli: i64,
    pub currency: String,
    pub unit: String,
    pub note: String,
    pub set_at: DateTime<Utc>,
    pub confirmed_at: Option<DateTime<Utc>>,
    /// Ledger rows that used this baseline; a used baseline can no longer be removed.
    pub used: i64,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct EntryView {
    pub id: Uuid,
    pub order_id: Uuid,
    pub po_number: String,
    pub request_number: String,
    pub description: String,
    pub ordered_on: NaiveDate,
    pub quantity: i32,
    pub unit_price_minor: i64,
    pub baseline_unit_price_milli: Option<i64>,
    pub currency: String,
    pub share_bps: Option<i32>,
    pub saving_minor: i64,
    pub fee_minor: i64,
    pub status: String,
    pub invoice_id: Option<Uuid>,
    pub invoice_number: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct CurrencyTotal {
    pub currency: String,
    pub saving_minor: i64,
    pub fee_minor: i64,
    /// Fees not yet on a Chuo-Sogo invoice.
    pub unbilled_fee_minor: i64,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct SavingsView {
    pub agreement: Option<AgreementView>,
    pub totals: Vec<CurrencyTotal>,
    pub entries: Vec<EntryView>,
    pub baselines: Vec<BaselineView>,
}

/// `admin`: the console sees draft invoices; the factory sees only issued ones (a fee on a
/// draft is still unbilled to the factory).
async fn load_view(tx: &mut Tx, admin: bool) -> ApiResult<SavingsView> {
    #[derive(sqlx::FromRow)]
    struct A {
        share_bps: i32,
        term_months: i32,
        starts_on: NaiveDate,
        ends_on: NaiveDate,
        notes: String,
        updated_at: DateTime<Utc>,
    }
    let agreement = sqlx::query_as::<_, A>(
        "SELECT share_bps, term_months, starts_on, ends_on, notes, updated_at
         FROM savings_agreements WHERE org_id = app_org_id()",
    )
    .fetch_optional(&mut **tx)
    .await?
    .map(|a| AgreementView {
        share_bps: a.share_bps,
        term_months: a.term_months,
        starts_on: a.starts_on,
        ends_on: a.ends_on,
        notes: a.notes,
        updated_at: a.updated_at,
    });
    // Suppliers are not shown here: the ledger is about prices, and the directory is
    // RLS-limited for customers anyway.
    let entries = sqlx::query_as::<_, EntryView>(
        "SELECT e.id, e.order_id, o.po_number, r.number AS request_number, ri.description,
                e.ordered_on, e.quantity, e.unit_price_minor, e.baseline_unit_price_milli,
                e.currency, e.share_bps, e.saving_minor, e.fee_minor, e.status,
                CASE WHEN $1 OR inv.status <> 'draft' THEN e.invoice_id END AS invoice_id,
                inv.number AS invoice_number
         FROM savings_entries e
         JOIN purchase_orders o ON o.id = e.order_id
         JOIN procurement_requests r ON r.id = o.request_id
         JOIN order_items oi ON oi.id = e.order_item_id
         JOIN request_items ri ON ri.id = oi.request_item_id
         LEFT JOIN issued_invoices inv ON inv.id = e.invoice_id
         WHERE e.org_id = app_org_id()
         ORDER BY e.ordered_on DESC, o.po_number, ri.line_no",
    )
    .bind(admin)
    .fetch_all(&mut **tx)
    .await?;
    let baselines = sqlx::query_as::<_, BaselineView>(
        "SELECT b.id, b.item_key, b.source, b.purchased_on, b.supplier_name, b.unit_price_milli,
                b.currency, b.unit, b.note, b.set_at, b.confirmed_at,
                (SELECT count(*) FROM savings_entries e WHERE e.baseline_id = b.id) AS used
         FROM savings_baselines b WHERE b.org_id = app_org_id()
         ORDER BY b.item_key",
    )
    .fetch_all(&mut **tx)
    .await?;
    let mut by_ccy: BTreeMap<String, CurrencyTotal> = BTreeMap::new();
    for e in entries.iter().filter(|e| e.status == "counted") {
        let t = by_ccy
            .entry(e.currency.clone())
            .or_insert_with(|| CurrencyTotal {
                currency: e.currency.clone(),
                saving_minor: 0,
                fee_minor: 0,
                unbilled_fee_minor: 0,
            });
        t.saving_minor = t.saving_minor.saturating_add(e.saving_minor);
        t.fee_minor = t.fee_minor.saturating_add(e.fee_minor);
        if e.invoice_id.is_none() {
            t.unbilled_fee_minor = t.unbilled_fee_minor.saturating_add(e.fee_minor);
        }
    }
    Ok(SavingsView {
        agreement,
        totals: by_ccy.into_values().collect(),
        entries,
        baselines,
    })
}

// ---------------------------------------------------------------------------------------
// Factory side.

async fn require_read(state: &AppState, p: &Principal, org_id: Uuid) -> ApiResult<Access> {
    let access = p.access(state, org_id).await?;
    if access == Access::None {
        return Err(ApiError::NotFound);
    }
    if rbac::allows(access, Permission::ViewFinance)
        || rbac::allows(access, Permission::ViewAllRequests)
    {
        Ok(access)
    } else {
        Err(ApiError::Forbidden)
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/savings",
    tag = "savings",
    params(("org_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Agreement, ledger, totals and baselines", body = SavingsView),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation", body = ErrorBody),
    )
)]
pub async fn get_savings(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
) -> ApiResult<Json<SavingsView>> {
    require_read(&state, &p, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let view = load_view(&mut tx, false).await?;
    tx.commit().await?;
    Ok(Json(view))
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/savings/baselines/{baseline_id}/confirm",
    tag = "savings",
    params(("org_id" = Uuid, Path), ("baseline_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Confirmed by the factory"),
        (status = 409, description = "Already confirmed", body = ErrorBody),
        (status = 403, description = "Only a factory approver confirms", body = ErrorBody),
        (status = 404, description = "Unknown baseline", body = ErrorBody),
    )
)]
pub async fn confirm_baseline(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, baseline_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    // The price we are paid against is the factory's to accept, not ours.
    let access = p
        .require(&state, org_id, Permission::ApproveRecommendation)
        .await?;
    if common::side(access)? != Side::Customer {
        return Err(ApiError::Forbidden);
    }
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let confirmed: Option<bool> = sqlx::query_scalar(
        "SELECT confirmed_at IS NOT NULL FROM savings_baselines WHERE id = $1 FOR UPDATE",
    )
    .bind(baseline_id)
    .fetch_optional(&mut *tx)
    .await?;
    match confirmed {
        None => return Err(ApiError::NotFound),
        Some(true) => return Err(ApiError::Conflict("already_confirmed")),
        Some(false) => {}
    }
    sqlx::query(
        "UPDATE savings_baselines SET confirmed_by = $2, confirmed_at = now() WHERE id = $1",
    )
    .bind(baseline_id)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    // Lines that waited for this confirmation are priced now.
    let baseline = sqlx::query_as::<_, BaselineRow>(&format!(
        "SELECT {BASELINE_COLUMNS} FROM savings_baselines WHERE id = $1"
    ))
    .bind(baseline_id)
    .fetch_one(&mut *tx)
    .await?;
    #[derive(sqlx::FromRow)]
    struct Waiting {
        id: Uuid,
        unit_price_minor: i64,
        quantity: i32,
        unit: String,
        currency: String,
        share_bps: i32,
    }
    let waiting = sqlx::query_as::<_, Waiting>(
        "SELECT e.id, e.unit_price_minor, e.quantity, ri.unit, e.currency, e.share_bps
         FROM savings_entries e
         JOIN order_items oi ON oi.id = e.order_item_id
         JOIN request_items ri ON ri.id = oi.request_item_id
         WHERE e.baseline_id = $1 AND e.status = 'unconfirmed_baseline'
           AND e.share_bps IS NOT NULL
         FOR UPDATE OF e",
    )
    .bind(baseline_id)
    .fetch_all(&mut *tx)
    .await?;
    let mut fee_total = 0i64;
    for w in &waiting {
        let currency = common::currency(&w.currency)?;
        let (status, saving, fee) = price_line(
            &baseline,
            w.unit_price_minor,
            w.quantity,
            &w.unit,
            currency,
            w.share_bps,
        )?;
        fee_total = fee_total.saturating_add(fee);
        sqlx::query(
            "UPDATE savings_entries SET status = $2, saving_minor = $3, fee_minor = $4 WHERE id = $1",
        )
        .bind(w.id)
        .bind(status)
        .bind(saving)
        .bind(fee)
        .execute(&mut *tx)
        .await?;
    }
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "savings.baseline_confirmed",
            "savings_baseline",
            baseline_id,
            json!({ "lines_priced": waiting.len(), "fee_minor": fee_total }),
        )
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------------------
// Admin console.

#[utoipa::path(
    operation_id = "admin_get_savings",
    get,
    path = "/api/admin/v1/orgs/{org_id}/savings",
    tag = "savings",
    params(("org_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Agreement, ledger, totals and baselines", body = SavingsView),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation", body = ErrorBody),
    )
)]
pub async fn admin_get_savings(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath(org_id): ApiPath<Uuid>,
) -> ApiResult<Json<SavingsView>> {
    let (mut tx, _, _, _) = billing::org_billing_tx(&state, &p, org_id).await?;
    let view = load_view(&mut tx, true).await?;
    tx.commit().await?;
    Ok(Json(view))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct AgreementInput {
    /// Our share of the saving in basis points; default 5000 (half).
    pub share_bps: Option<u32>,
    /// Default 36.
    pub term_months: Option<u32>,
    pub starts_on: NaiveDate,
    #[serde(default)]
    pub notes: String,
}

#[utoipa::path(
    put,
    path = "/api/admin/v1/orgs/{org_id}/savings/agreement",
    tag = "savings",
    params(("org_id" = Uuid, Path)),
    request_body = AgreementInput,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Saved"),
        (status = 409, description = "Terms are fixed once savings were recorded", body = ErrorBody),
        (status = 422, description = "Invalid share, term or date", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation", body = ErrorBody),
    )
)]
pub async fn put_agreement(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<AgreementInput>,
) -> ApiResult<StatusCode> {
    let share_bps = body.share_bps.unwrap_or(savings::DEFAULT_SHARE_BPS);
    let term_months = body.term_months.unwrap_or(savings::DEFAULT_TERM_MONTHS);
    savings::validate_terms(share_bps, term_months).map_err(|e| match e {
        SavingsError::InvalidTerm => ApiError::Validation("term_months"),
        _ => ApiError::Validation("share_bps"),
    })?;
    let notes = common::text(&body.notes, 2000, true, "notes")?;
    let ends_on = naive(
        savings::term_end(day(body.starts_on)?, term_months)
            .ok_or(ApiError::Validation("starts_on"))?,
    )?;
    let (mut tx, platform, _, _) = billing::org_billing_tx(&state, &p, org_id).await?;
    let kind: String = sqlx::query_scalar("SELECT kind FROM organisations WHERE id = $1")
        .bind(org_id)
        .fetch_one(&mut *tx)
        .await?;
    if kind != "customer" {
        return Err(ApiError::Validation("not_a_customer_org"));
    }
    // Ledger rows were computed under the current terms; changing them afterwards would
    // rewrite what the factory already agreed to pay.
    // Lines recorded while there were no terms never earn a fee and do not fix them.
    let used: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM savings_entries WHERE status <> 'no_agreement')",
    )
    .fetch_one(&mut *tx)
    .await?;
    if used {
        return Err(ApiError::Conflict("terms_fixed"));
    }
    sqlx::query(
        "INSERT INTO savings_agreements (org_id, share_bps, term_months, starts_on, ends_on,
             notes, updated_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (org_id) DO UPDATE SET share_bps = $2, term_months = $3, starts_on = $4,
             ends_on = $5, notes = $6, updated_by = $7, updated_at = now()",
    )
    .bind(org_id)
    .bind(i32::try_from(share_bps).map_err(ApiError::internal)?)
    .bind(i32::try_from(term_months).map_err(ApiError::internal)?)
    .bind(body.starts_on)
    .bind(ends_on)
    .bind(&notes)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    let data = json!({ "share_bps": share_bps, "term_months": term_months,
                       "starts_on": body.starts_on, "ends_on": ends_on });
    for target in [platform, org_id] {
        Audit {
            org_id: target,
            p: &p,
        }
        .record(
            &mut tx,
            "savings.agreement_saved",
            "organisation",
            org_id,
            data.clone(),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct AgreedBaselineInput {
    /// Part number and/or description of the item, as on the factory's request.
    #[serde(default)]
    pub mpn: String,
    #[serde(default)]
    pub description: String,
    /// The price the factory paid before, in thousandths of a minor unit.
    pub unit_price_milli: i64,
    pub currency: Option<String>,
    #[serde(default)]
    pub unit: String,
    /// Where the number comes from (e.g. "factory's invoice 2026-03, no. 1234").
    pub note: String,
}

#[utoipa::path(
    post,
    path = "/api/admin/v1/orgs/{org_id}/savings/baselines",
    tag = "savings",
    params(("org_id" = Uuid, Path)),
    request_body = AgreedBaselineInput,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Recorded; counts after the factory confirms it", body = crate::api::suppliers::IdResponse),
        (status = 409, description = "The item already has a baseline", body = ErrorBody),
        (status = 422, description = "Invalid input", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
    )
)]
pub async fn add_agreed_baseline(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<AgreedBaselineInput>,
) -> ApiResult<(StatusCode, Json<crate::api::suppliers::IdResponse>)> {
    let key =
        history::item_key(&body.mpn, &body.description).ok_or(ApiError::Validation("item"))?;
    if !(0..=history::MAX_PRICE_MILLI).contains(&body.unit_price_milli) {
        return Err(ApiError::Validation("unit_price_milli"));
    }
    let currency: Currency = common::currency(body.currency.as_deref().unwrap_or("JPY"))?;
    let unit = common::text(&body.unit, 16, false, "unit")?;
    let note = common::required(&body.note, 1000, true, "note")?;
    let (mut tx, _, _, _) = billing::org_billing_tx(&state, &p, org_id).await?;
    let groups = key_groups(&mut tx, std::slice::from_ref(&key)).await?;
    let group = ordered_group(&key, groups.get(&key).map_or(&[][..], |g| g.as_slice()));
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM savings_baselines WHERE item_key = ANY($1))",
    )
    .bind(&group)
    .fetch_one(&mut *tx)
    .await?;
    if exists {
        return Err(ApiError::Conflict("baseline_exists"));
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO savings_baselines (id, org_id, item_key, source, unit_price_milli, currency,
             unit, note, set_by)
         VALUES ($1, $2, $3, 'agreed', $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(org_id)
    .bind(&group[0])
    .bind(body.unit_price_milli)
    .bind(currency.code())
    .bind(&unit)
    .bind(&note)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "savings.baseline_proposed",
            "savings_baseline",
            id,
            json!({ "item_key": group[0], "unit_price_milli": body.unit_price_milli,
                    "currency": currency.code() }),
        )
        .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(crate::api::suppliers::IdResponse { id }),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/admin/v1/orgs/{org_id}/savings/baselines/{baseline_id}",
    tag = "savings",
    params(("org_id" = Uuid, Path), ("baseline_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Removed"),
        (status = 409, description = "Already used or confirmed", body = ErrorBody),
        (status = 404, description = "Unknown baseline", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
    )
)]
pub async fn delete_baseline(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath((org_id, baseline_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    let (mut tx, _, _, _) = billing::org_billing_tx(&state, &p, org_id).await?;
    let row: Option<(bool, bool)> = sqlx::query_as(
        "SELECT confirmed_at IS NOT NULL,
                EXISTS (SELECT 1 FROM savings_entries e WHERE e.baseline_id = b.id)
         FROM savings_baselines b WHERE id = $1 FOR UPDATE",
    )
    .bind(baseline_id)
    .fetch_optional(&mut *tx)
    .await?;
    match row {
        None => return Err(ApiError::NotFound),
        // Once the factory accepted it, or a fee was computed with it, it stays.
        Some((true, _) | (_, true)) => return Err(ApiError::Conflict("baseline_fixed")),
        Some(_) => {}
    }
    sqlx::query("DELETE FROM savings_baselines WHERE id = $1")
        .bind(baseline_id)
        .execute(&mut *tx)
        .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "savings.baseline_removed",
            "savings_baseline",
            baseline_id,
            json!({}),
        )
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct SavingsInvoiceInput {
    /// Bill fees of orders placed up to and including this day.
    pub through: NaiveDate,
    pub currency: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/admin/v1/orgs/{org_id}/savings/invoice",
    tag = "savings",
    params(("org_id" = Uuid, Path)),
    request_body = SavingsInvoiceInput,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Draft invoice with one line per order", body = CreatedInvoice),
        (status = 409, description = "Nothing to bill", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation", body = ErrorBody),
    )
)]
pub async fn create_savings_invoice(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<SavingsInvoiceInput>,
) -> ApiResult<(StatusCode, Json<CreatedInvoice>)> {
    let currency = common::currency(body.currency.as_deref().unwrap_or("JPY"))?;
    let (mut tx, platform, issuer, org_name) = billing::org_billing_tx(&state, &p, org_id).await?;
    #[derive(sqlx::FromRow)]
    struct Open {
        order_id: Uuid,
        po_number: String,
        first_on: NaiveDate,
        lines: i64,
        saving_minor: i64,
        fee_minor: i64,
        share_bps: i32,
    }
    // Locks the open rows so two drafts cannot bill the same fee.
    let open = sqlx::query_as::<_, Open>(
        "SELECT e.order_id, o.po_number, min(e.ordered_on) AS first_on, count(*) AS lines,
                sum(e.saving_minor)::bigint AS saving_minor, sum(e.fee_minor)::bigint AS fee_minor,
                min(e.share_bps) AS share_bps
         FROM savings_entries e JOIN purchase_orders o ON o.id = e.order_id
         WHERE e.id IN (SELECT id FROM savings_entries
                        WHERE status = 'counted' AND invoice_id IS NULL
                          AND currency = $1 AND ordered_on <= $2 FOR UPDATE)
         GROUP BY e.order_id, o.po_number
         ORDER BY first_on, o.po_number
         LIMIT $3",
    )
    .bind(currency.code())
    .bind(body.through)
    .bind(i64::try_from(MAX_ORDERS_PER_INVOICE).map_err(ApiError::internal)?)
    .fetch_all(&mut *tx)
    .await?;
    if open.is_empty() {
        return Err(ApiError::Conflict("nothing_to_bill"));
    }
    let money = |m: i64| Money::from_minor(m, currency);
    let lines: Vec<LineInput> = open
        .iter()
        .map(|o| LineInput {
            description: format!(
                "Savings share {}% — PO {} ({}, {} line(s), saving {})",
                percent_text(o.share_bps),
                o.po_number,
                o.first_on,
                o.lines,
                amount_text(money(o.saving_minor))
            ),
            quantity: 1,
            unit: String::new(),
            unit_price_minor: o.fee_minor,
            tax_rate_bps: FEE_TAX_BPS,
        })
        .collect();
    let input = InvoiceInput {
        currency: Some(currency.code().to_string()),
        recipient_name: None,
        recipient_address: String::new(),
        due_date: None,
        request_id: None,
        notes: format!(
            "Savings share for orders placed through {}. The saving is the price paid before \
             minus the price now, times the quantity; no saving, no fee.",
            body.through
        ),
        lines,
    };
    let id =
        billing::insert_draft(&mut tx, &p, platform, org_id, &org_name, issuer, &input).await?;
    let orders: Vec<Uuid> = open.iter().map(|o| o.order_id).collect();
    sqlx::query(
        "UPDATE savings_entries SET invoice_id = $1
         WHERE order_id = ANY($2) AND status = 'counted' AND invoice_id IS NULL
           AND currency = $3 AND ordered_on <= $4",
    )
    .bind(id)
    .bind(&orders)
    .bind(currency.code())
    .bind(body.through)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(CreatedInvoice { id })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts() {
        assert_eq!(
            amount_text(Money::from_minor(1_234_567, Currency::Jpy)),
            "JPY 1,234,567"
        );
        assert_eq!(
            amount_text(Money::from_minor(500, Currency::Jpy)),
            "JPY 500"
        );
        assert_eq!(
            amount_text(Money::from_minor(123_405, Currency::Usd)),
            "USD 1,234.05"
        );
    }

    #[test]
    fn percents() {
        assert_eq!(percent_text(5000), "50");
        assert_eq!(percent_text(1250), "12.5");
        assert_eq!(percent_text(3333), "33.33");
        assert_eq!(percent_text(5), "0.05");
    }

    #[test]
    fn group_order() {
        let g = ordered_group("m:A", &["m:B".into(), "m:A".into()]);
        assert_eq!(g, vec!["m:A".to_string(), "m:B".to_string()]);
    }
}
