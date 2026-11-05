//! `supplier_watch` (L6): a daily sweep per organisation of the compliance documents of its
//! approved suppliers (expiring within 30 / 7 days, or expired) and of last month's
//! scorecards. Findings are proposals only:
//! - `request_document_update` → on approval, a draft to the supplier in the outbox (test
//!   mode: recorded, never sent by the server), ja or en.
//! - `flag_supplier_review` → on approval, a task for our staff. The customer's own
//!   approval of the supplier is never changed: people decide that.
//!
//! Scorecards are computed on read ([`scorecards`]) from the customer's own records; see
//! migrations/0027_supplier_watch.sql for why nothing is stored.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use chrono::{Datelike, NaiveDate, Utc};
use futures_util::future::BoxFuture;
use procure_domain::agents::Severity;
use procure_domain::compliance::{self, DocKind, Expiry};
use procure_domain::rbac::Permission;
use procure_domain::scorecard::{self, Inputs, Scorecard, Thresholds};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{Actor, Agent, AgentCx, AgentError, AgentException, AgentRun, NewException, Outcome};
use crate::api::common::{self, Audit};
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiResult};

pub const NAME: &str = "supplier_watch";
pub const DOC_KEY_PREFIX: &str = "supplier_watch:doc:";
const DAY_SECONDS: u64 = 86_400;
/// `num_days_from_ce` of 1970-01-01.
const EPOCH_CE_DAYS: i64 = 719_163;

pub struct SupplierWatch;

/// The Japan-time calendar date now.
pub fn jst_today() -> NaiveDate {
    from_day(compliance::jst_day(Utc::now().timestamp())).unwrap_or_default()
}

fn to_day(d: NaiveDate) -> i64 {
    i64::from(d.num_days_from_ce()) - EPOCH_CE_DAYS
}

fn from_day(day: i64) -> Option<NaiveDate> {
    NaiveDate::from_num_days_from_ce_opt(i32::try_from(day + EPOCH_CE_DAYS).ok()?)
}

/// First day of `d`'s month.
pub fn month_start(d: NaiveDate) -> NaiveDate {
    d.with_day(1).unwrap_or(d)
}

/// First day of the month after the one starting at `start`.
pub fn next_month(start: NaiveDate) -> NaiveDate {
    start
        .checked_add_months(chrono::Months::new(1))
        .unwrap_or(start)
}

fn prev_month(start: NaiveDate) -> NaiveDate {
    start
        .checked_sub_months(chrono::Months::new(1))
        .unwrap_or(start)
}

/// `YYYY-MM` → first day of that month.
pub fn parse_month(raw: &str) -> Option<NaiveDate> {
    let (y, m) = raw.split_once('-')?;
    if y.len() != 4 || m.len() != 2 {
        return None;
    }
    NaiveDate::from_ymd_opt(y.parse().ok()?, m.parse().ok()?, 1)
}

/// One supplier's scorecard for one customer and month.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct SupplierScorecard {
    pub supplier_id: Uuid,
    pub supplier_name: String,
    /// `YYYY-MM` (Japan time).
    pub month: String,
    /// Orders promised in the month delivered (first delivery) on or before the promised date.
    pub on_time_bps: Option<u32>,
    pub deliveries_measured: u32,
    /// Invoices of the month with at least one discrepancy.
    pub discrepancy_bps: Option<u32>,
    pub invoices: u32,
    /// RFQs sent in the month that the supplier answered (quotation or decline).
    pub reply_rate_bps: Option<u32>,
    pub rfqs_received: u32,
    pub median_reply_hours: Option<u32>,
    /// Mean price-rank score in the month's final comparisons (10 000 = always cheapest).
    pub price_score_bps: Option<u32>,
    pub comparisons_ranked: u32,
}

impl SupplierScorecard {
    fn card(&self) -> Scorecard {
        Scorecard {
            on_time_bps: self.on_time_bps,
            deliveries_measured: self.deliveries_measured,
            discrepancy_bps: self.discrepancy_bps,
            invoices: self.invoices,
            reply_rate_bps: self.reply_rate_bps,
            rfqs_received: self.rfqs_received,
            median_reply_hours: self.median_reply_hours,
            price_score_bps: self.price_score_bps,
            comparisons_ranked: self.comparisons_ranked,
        }
    }
}

fn count(n: i64) -> u32 {
    u32::try_from(n.max(0)).unwrap_or(u32::MAX)
}

/// Scorecards of every supplier with activity in the month starting at `month`, from the
/// records of `org_id` (the transaction is in that organisation's context; RLS limits it to
/// that tenant). Orders not yet delivered count as late only once `today` (Japan time) is
/// past their promised date.
pub async fn scorecards(
    tx: &mut Tx,
    org_id: Uuid,
    month: NaiveDate,
    today: NaiveDate,
) -> Result<Vec<SupplierScorecard>, sqlx::Error> {
    let end = next_month(month);
    let mut inputs: BTreeMap<Uuid, Inputs> = BTreeMap::new();

    let rows: Vec<(Uuid, i64, i64)> = sqlx::query_as(
        "SELECT o.supplier_id,
                count(*) FILTER (WHERE f.first_day <= o.promised_on),
                count(*) FILTER (WHERE f.first_day > o.promised_on
                                    OR (f.first_day IS NULL AND o.promised_on < $4))
         FROM purchase_orders o
         LEFT JOIN LATERAL (
             SELECT min((d.received_at AT TIME ZONE 'Asia/Tokyo')::date) AS first_day
             FROM deliveries d WHERE d.order_id = o.id AND d.reversed_at IS NULL) f ON true
         WHERE o.org_id = $1 AND o.promised_on >= $2 AND o.promised_on < $3
         GROUP BY o.supplier_id",
    )
    .bind(org_id)
    .bind(month)
    .bind(end)
    .bind(today)
    .fetch_all(&mut **tx)
    .await?;
    for (s, on_time, late) in rows {
        let i = inputs.entry(s).or_default();
        i.deliveries_on_time = count(on_time);
        i.deliveries_late = count(late);
    }

    let rows: Vec<(Uuid, i64, i64)> = sqlx::query_as(
        "SELECT i.supplier_id, count(*),
                count(*) FILTER (WHERE EXISTS (SELECT 1 FROM discrepancies x
                                               WHERE x.invoice_id = i.id))
         FROM invoices i
         WHERE i.org_id = $1 AND i.invoice_date >= $2 AND i.invoice_date < $3
         GROUP BY i.supplier_id",
    )
    .bind(org_id)
    .bind(month)
    .bind(end)
    .fetch_all(&mut **tx)
    .await?;
    for (s, n, with) in rows {
        let i = inputs.entry(s).or_default();
        i.invoices = count(n);
        i.invoices_with_discrepancy = count(with);
    }

    let rows: Vec<(Uuid, bool, Option<i64>)> = sqlx::query_as(
        "SELECT r.supplier_id,
                r.status IN ('responded', 'declined')
                    OR EXISTS (SELECT 1 FROM quotations q WHERE q.rfq_recipient_id = r.id),
                (SELECT greatest(0, floor(extract(epoch FROM min(q.received_at) - f.sent_at)
                                          / 3600))::bigint
                 FROM quotations q WHERE q.rfq_recipient_id = r.id)
         FROM rfq_recipients r JOIN rfqs f ON f.id = r.rfq_id
         WHERE r.org_id = $1 AND f.sent_at IS NOT NULL
           AND (f.sent_at AT TIME ZONE 'Asia/Tokyo')::date >= $2
           AND (f.sent_at AT TIME ZONE 'Asia/Tokyo')::date < $3",
    )
    .bind(org_id)
    .bind(month)
    .bind(end)
    .fetch_all(&mut **tx)
    .await?;
    for (s, answered, hours) in rows {
        let i = inputs.entry(s).or_default();
        i.rfqs_received = i.rfqs_received.saturating_add(1);
        if answered {
            i.rfqs_answered = i.rfqs_answered.saturating_add(1);
        }
        if let Some(h) = hours {
            i.reply_hours.push(count(h));
        }
    }

    let rows: Vec<(Uuid, i64, i64)> = sqlx::query_as(
        "WITH last AS (
             SELECT DISTINCT ON (c.rfq_id) c.results
             FROM comparison_runs c
             WHERE c.org_id = $1 AND (c.ran_at AT TIME ZONE 'Asia/Tokyo')::date >= $2
               AND (c.ran_at AT TIME ZONE 'Asia/Tokyo')::date < $3
             ORDER BY c.rfq_id, c.ran_at DESC, c.id DESC)
         SELECT (e ->> 'supplier_id')::uuid, (e ->> 'rank')::bigint,
                (SELECT count(*) FROM jsonb_array_elements(l.results) x
                 WHERE x ->> 'rank' IS NOT NULL)
         FROM last l, jsonb_array_elements(l.results) e
         WHERE e ->> 'rank' IS NOT NULL",
    )
    .bind(org_id)
    .bind(month)
    .bind(end)
    .fetch_all(&mut **tx)
    .await?;
    for (s, rank, of) in rows {
        inputs
            .entry(s)
            .or_default()
            .price_ranks
            .push((count(rank), count(of)));
    }

    let ids: Vec<Uuid> = inputs.keys().copied().collect();
    let names: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, name FROM suppliers WHERE id = ANY($1)")
            .bind(&ids)
            .fetch_all(&mut **tx)
            .await?;
    let names: BTreeMap<Uuid, String> = names.into_iter().collect();
    let label = month.format("%Y-%m").to_string();
    let mut out: Vec<SupplierScorecard> = inputs
        .into_iter()
        .map(|(supplier_id, i)| {
            let c = scorecard::compute(&i);
            SupplierScorecard {
                supplier_id,
                supplier_name: names.get(&supplier_id).cloned().unwrap_or_default(),
                month: label.clone(),
                on_time_bps: c.on_time_bps,
                deliveries_measured: c.deliveries_measured,
                discrepancy_bps: c.discrepancy_bps,
                invoices: c.invoices,
                reply_rate_bps: c.reply_rate_bps,
                rfqs_received: c.rfqs_received,
                median_reply_hours: c.median_reply_hours,
                price_score_bps: c.price_score_bps,
                comparisons_ranked: c.comparisons_ranked,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        a.supplier_name
            .cmp(&b.supplier_name)
            .then(a.supplier_id.cmp(&b.supplier_id))
    });
    Ok(out)
}

#[derive(sqlx::FromRow)]
struct WatchedDoc {
    id: Uuid,
    supplier_id: Uuid,
    supplier_name: String,
    kind: String,
    expires_on: NaiveDate,
}

/// The documents an organisation's sweep looks at: visible to it (customer-visible, by RLS),
/// of suppliers it approved, not retired, with an expiry date, and the newest of their kind.
async fn watched_docs(tx: &mut Tx, org_id: Uuid) -> Result<Vec<WatchedDoc>, sqlx::Error> {
    sqlx::query_as::<_, WatchedDoc>(
        "SELECT d.id, d.supplier_id, s.name AS supplier_name, d.kind, d.expires_on
         FROM supplier_compliance_docs d JOIN suppliers s ON s.id = d.supplier_id
         WHERE d.retired_at IS NULL AND d.expires_on IS NOT NULL AND s.status <> 'inactive'
           AND d.supplier_id IN (SELECT a.supplier_id FROM customer_supplier_approvals a
                                 WHERE a.org_id = $1 AND a.status = 'approved')
           AND (d.kind = 'other' OR NOT EXISTS (
                SELECT 1 FROM supplier_compliance_docs n
                WHERE n.supplier_id = d.supplier_id AND n.kind = d.kind AND n.id <> d.id
                  AND n.retired_at IS NULL
                  AND (n.expires_on IS NULL OR n.expires_on > d.expires_on
                       OR (n.expires_on = d.expires_on AND n.id > d.id))))
         ORDER BY d.expires_on, d.id",
    )
    .bind(org_id)
    .fetch_all(&mut **tx)
    .await
}

fn doc_exception(d: &WatchedDoc, today: i64) -> Option<NewException> {
    let state = compliance::expiry(to_day(d.expires_on), today);
    let window = state.window()?;
    let (kind, severity, title) = match state {
        Expiry::Expired { .. } => (
            "document_expired",
            Severity::Critical,
            "supplier.documentExpired",
        ),
        Expiry::Urgent { .. } => (
            "document_expiring",
            Severity::Warning,
            "supplier.documentExpiring",
        ),
        _ => (
            "document_expiring",
            Severity::Info,
            "supplier.documentExpiring",
        ),
    };
    // One finding per document, expiry date and window: a renewal or the next window is a
    // new finding, and the previous one is closed by the sweep.
    let key = format!("{DOC_KEY_PREFIX}{}:{}:{window}", d.id, d.expires_on);
    let mut ex = NewException::new(kind, severity, title, &key);
    ex.subject_type = Some("supplier_compliance_doc".into());
    ex.subject_id = Some(d.id);
    ex.params = json!({
        "supplier_id": d.supplier_id,
        "supplier_name": d.supplier_name,
        "doc_kind": d.kind,
        "expires_on": d.expires_on,
        "window": window,
    });
    ex.proposed_action = Some(json!({ "type": "request_document_update", "doc_id": d.id }));
    Some(ex)
}

fn review_exception(
    card: &SupplierScorecard,
    previous: Option<&SupplierScorecard>,
) -> Option<NewException> {
    let reasons = scorecard::review_reasons(
        &card.card(),
        previous.map(SupplierScorecard::card).as_ref(),
        &Thresholds::default(),
    );
    if reasons.is_empty() {
        return None;
    }
    let codes: Vec<&str> = reasons.iter().map(|r| r.code()).collect();
    let key = format!("supplier_watch:review:{}:{}", card.supplier_id, card.month);
    let mut ex = NewException::new(
        "scorecard_below_threshold",
        Severity::Warning,
        "supplier.scorecardReview",
        &key,
    );
    ex.subject_type = Some("supplier".into());
    ex.subject_id = Some(card.supplier_id);
    ex.params = json!({
        "supplier_id": card.supplier_id,
        "supplier_name": card.supplier_name,
        "month": card.month,
        "reasons": codes,
        "on_time_bps": card.on_time_bps,
        "discrepancy_bps": card.discrepancy_bps,
        "reply_rate_bps": card.reply_rate_bps,
        "previous_on_time_bps": previous.and_then(|p| p.on_time_bps),
        "previous_discrepancy_bps": previous.and_then(|p| p.discrepancy_bps),
        "previous_reply_rate_bps": previous.and_then(|p| p.reply_rate_bps),
    });
    ex.proposed_action = Some(json!({
        "type": "flag_supplier_review",
        "supplier_id": card.supplier_id,
        "month": card.month,
        "reasons": codes,
    }));
    Some(ex)
}

/// The day the sweep runs for: today in Japan, or `input.as_of` (`YYYY-MM-DD`) on a run a
/// person started by hand (to check a past day; it only ever produces proposals).
fn as_of(run: &AgentRun) -> Result<NaiveDate, AgentError> {
    match run.input.get("as_of").and_then(Value::as_str) {
        Some(raw) if run.trigger == "manual" => {
            NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| AgentError::Fatal("as_of"))
        }
        _ => Ok(jst_today()),
    }
}

async fn sweep(cx: &AgentCx, run: &AgentRun) -> Result<Outcome, AgentError> {
    let today = as_of(run)?;
    let today_day = to_day(today);
    let mut tx = cx.tx().await?;
    let docs = watched_docs(&mut tx, cx.org_id).await?;
    let mut exceptions: Vec<NewException> = docs
        .iter()
        .filter_map(|d| doc_exception(d, today_day))
        .collect();

    // Findings that no longer hold (renewed, retired, expiring → expired, hidden from the
    // customer, supplier no longer approved) are closed as resolved elsewhere.
    let current: HashSet<&str> = exceptions.iter().map(|e| e.dedupe_key.as_str()).collect();
    let open: Vec<String> = sqlx::query_scalar(
        "SELECT dedupe_key FROM agent_exceptions
         WHERE org_id = $1 AND agent = $2 AND status = 'open' AND starts_with(dedupe_key, $3)",
    )
    .bind(cx.org_id)
    .bind(NAME)
    .bind(DOC_KEY_PREFIX)
    .fetch_all(&mut *tx)
    .await?;
    let mut resolved = 0;
    for key in open.iter().filter(|k| !current.contains(k.as_str())) {
        if super::resolve_elsewhere(&mut tx, cx.org_id, key)
            .await?
            .is_some()
        {
            resolved += 1;
        }
    }

    // Last full month against the month before.
    let month = prev_month(month_start(today));
    let cards = scorecards(&mut tx, cx.org_id, month, today).await?;
    let before = scorecards(&mut tx, cx.org_id, prev_month(month), today).await?;
    tx.commit().await?;
    let before: BTreeMap<Uuid, &SupplierScorecard> =
        before.iter().map(|c| (c.supplier_id, c)).collect();
    let mut flagged = 0;
    for card in &cards {
        if let Some(ex) = review_exception(card, before.get(&card.supplier_id).copied()) {
            exceptions.push(ex);
            flagged += 1;
        }
    }
    // A finding a person already decided is not proposed again (a new expiry date, window
    // or month is a new finding with its own key).
    let keys: Vec<String> = exceptions.iter().map(|e| e.dedupe_key.clone()).collect();
    let decided: Vec<String> = {
        let mut tx = cx.tx().await?;
        let d = sqlx::query_scalar(
            "SELECT DISTINCT dedupe_key FROM agent_exceptions
             WHERE org_id = $1 AND agent = $2 AND dedupe_key = ANY($3)
               AND status IN ('approved', 'edited', 'rejected')",
        )
        .bind(cx.org_id)
        .bind(NAME)
        .bind(&keys)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        d
    };
    exceptions.retain(|e| !decided.contains(&e.dedupe_key));
    let mut out = Outcome::new(json!({
        "as_of": today,
        "documents_watched": docs.len(),
        "resolved": resolved,
        "already_decided": decided.len(),
        "scorecard_month": month.format("%Y-%m").to_string(),
        "suppliers_scored": cards.len(),
        "suppliers_flagged": flagged,
        "proposed": exceptions.len(),
    }));
    out.exceptions = exceptions;
    Ok(out)
}

/// Who may act on a supplier finding: anyone in the organisation who works with suppliers
/// (the customer's supplier approver, or our assigned staff).
fn require_supplier_work(actor: &Actor<'_>) -> ApiResult<()> {
    [
        Permission::ApproveSuppliers,
        Permission::ManageRfqs,
        Permission::ManageSuppliers,
    ]
    .into_iter()
    .find_map(|p| actor.require(p).ok())
    .ok_or(ApiError::Forbidden)
}

fn kind_name(kind: DocKind, ja: bool) -> &'static str {
    match (kind, ja) {
        (DocKind::Iso9001, true) => "ISO 9001 認証書",
        (DocKind::Iso9001, false) => "ISO 9001 certificate",
        (DocKind::Iso14001, true) => "ISO 14001 認証書",
        (DocKind::Iso14001, false) => "ISO 14001 certificate",
        (DocKind::Iatf16949, true) => "IATF 16949 認証書",
        (DocKind::Iatf16949, false) => "IATF 16949 certificate",
        (DocKind::QualifiedInvoiceRegistration, true) => "適格請求書発行事業者登録通知書",
        (DocKind::QualifiedInvoiceRegistration, false) => {
            "qualified invoice issuer registration notice"
        }
        (DocKind::AntiSocialCheck, true) => "反社会的勢力に関する確認書",
        (DocKind::AntiSocialCheck, false) => "anti-social forces declaration",
        (DocKind::Insurance, true) => "保険証券",
        (DocKind::Insurance, false) => "insurance certificate",
        (DocKind::Nda, true) => "秘密保持契約書",
        (DocKind::Nda, false) => "non-disclosure agreement",
        (DocKind::Other, true) => "ご提出書類",
        (DocKind::Other, false) => "document",
    }
}

/// The request to the supplier, in Japanese or English.
pub fn document_request_text(
    ja: bool,
    supplier: &str,
    kind: DocKind,
    title: &str,
    expires_on: NaiveDate,
    expired: bool,
    note: &str,
) -> (String, String) {
    let what = if title.is_empty() {
        kind_name(kind, ja).to_string()
    } else {
        format!("{}（{title}）", kind_name(kind, ja))
    };
    let note = if note.is_empty() {
        String::new()
    } else {
        format!("\n{note}\n")
    };
    if ja {
        let date = expires_on.format("%Y年%m月%d日");
        let state = if expired {
            format!("{date}で有効期限が切れております")
        } else {
            format!("{date}に有効期限を迎えます")
        };
        (
            format!("{} 更新版ご提出のお願い", kind_name(kind, true)),
            format!(
                "{supplier} 御中\n\nいつもお世話になっております。\n\
                 弊社にてお預かりしている貴社の{what}は、{state}。\n\
                 お手数ですが、更新版をご提出いただけますようお願い申し上げます。\n{note}\n\
                 何卒よろしくお願いいたします。\n"
            ),
        )
    } else {
        let date = expires_on.format("%Y-%m-%d");
        let state = if expired {
            format!("expired on {date}")
        } else {
            format!("expires on {date}")
        };
        (
            format!("Request for an updated {}", kind_name(kind, false)),
            format!(
                "Dear {supplier},\n\nThe {what} we hold on file for your company {state}.\n\
                 Could you please send us the updated version?\n{note}\nThank you.\n"
            ),
        )
    }
}

#[derive(sqlx::FromRow)]
struct DocForMail {
    supplier_id: Uuid,
    supplier_name: String,
    supplier_email: Option<String>,
    country: String,
    kind: String,
    title: String,
    expires_on: Option<NaiveDate>,
}

async fn request_document_update(
    tx: &mut Tx,
    ex: &AgentException,
    action: &Value,
    actor: &Actor<'_>,
) -> ApiResult<()> {
    require_supplier_work(actor)?;
    // An edit may change the language and add a note, never which document it is about.
    let doc_id = action
        .get("doc_id")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<Uuid>().ok())
        .ok_or(ApiError::Validation("doc_id"))?;
    if Some(doc_id) != ex.subject_id {
        return Err(ApiError::Validation("doc_id"));
    }
    let note = common::text(
        action.get("note").and_then(Value::as_str).unwrap_or(""),
        1000,
        true,
        "note",
    )?;
    let doc = sqlx::query_as::<_, DocForMail>(
        "SELECT d.supplier_id, s.name AS supplier_name, s.email::text AS supplier_email,
                s.country, d.kind, d.title, d.expires_on
         FROM supplier_compliance_docs d JOIN suppliers s ON s.id = d.supplier_id
         WHERE d.id = $1",
    )
    .bind(doc_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    let ja = match action.get("lang").and_then(Value::as_str) {
        None => doc.country == "JP",
        Some("ja") => true,
        Some("en") => false,
        Some(_) => return Err(ApiError::Validation("lang")),
    };
    let kind = DocKind::from_code(&doc.kind)
        .ok_or_else(|| ApiError::internal(std::io::Error::other("unknown document kind")))?;
    let expires_on = doc
        .expires_on
        .ok_or(ApiError::Conflict("document_has_no_expiry"))?;
    let expired = expires_on < jst_today();
    let (subject, body) = document_request_text(
        ja,
        &doc.supplier_name,
        kind,
        &doc.title,
        expires_on,
        expired,
        &note,
    );
    let outbox_id = Uuid::now_v7();
    // Test mode: recorded for our staff to send; the server never sends it.
    sqlx::query(
        "INSERT INTO outbox (id, org_id, rfq_recipient_id, channel, to_address, subject, body,
                             kind, supplier_id, compliance_doc_id)
         VALUES ($1, $2, NULL, 'test', $3, $4, $5, 'document_request', $6, $7)",
    )
    .bind(outbox_id)
    .bind(ex.org_id)
    .bind(&doc.supplier_email)
    .bind(&subject)
    .bind(&body)
    .bind(doc.supplier_id)
    .bind(doc_id)
    .execute(&mut **tx)
    .await?;
    Audit {
        org_id: ex.org_id,
        p: actor.principal,
    }
    .record(
        tx,
        "supplier_watch.document_update_requested",
        "outbox",
        outbox_id,
        json!({ "exception": ex.id, "doc_id": doc_id, "supplier_id": doc.supplier_id,
                "lang": if ja { "ja" } else { "en" }, "channel": "test" }),
    )
    .await
}

async fn flag_supplier_review(
    tx: &mut Tx,
    ex: &AgentException,
    action: &Value,
    actor: &Actor<'_>,
) -> ApiResult<()> {
    require_supplier_work(actor)?;
    let supplier_id = action
        .get("supplier_id")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<Uuid>().ok())
        .ok_or(ApiError::Validation("supplier_id"))?;
    if Some(supplier_id) != ex.subject_id {
        return Err(ApiError::Validation("supplier_id"));
    }
    let month = ex.params.get("month").and_then(Value::as_str).unwrap_or("");
    let name: String = sqlx::query_scalar("SELECT name FROM suppliers WHERE id = $1")
        .bind(supplier_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    let reasons = ex
        .params
        .get("reasons")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let title: String = format!("Review supplier {name} ({month}): {reasons}")
        .chars()
        .take(200)
        .collect();
    // An automatic task for our staff (created_by NULL), as workflow transitions open them:
    // the staff scope is on only for this insert.
    let prev: String =
        sqlx::query_scalar("SELECT coalesce(current_setting('app.staff', true), '')")
            .fetch_one(&mut **tx)
            .await?;
    db::enable_staff_scope(tx).await?;
    let task_id: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO tasks (id, org_id, kind, title, priority, due_at)
         SELECT $1, $2, 'exception', $3, 'normal', now() + interval '3 days'
         WHERE NOT EXISTS (SELECT 1 FROM tasks
                           WHERE org_id = $2 AND kind = 'exception' AND title = $3
                             AND created_by IS NULL AND status IN ('open', 'in_progress'))
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(ex.org_id)
    .bind(&title)
    .fetch_optional(&mut **tx)
    .await?;
    sqlx::query("SELECT set_config('app.staff', $1, true)")
        .bind(&prev)
        .execute(&mut **tx)
        .await?;
    Audit {
        org_id: ex.org_id,
        p: actor.principal,
    }
    .record(
        tx,
        "supplier_watch.review_flagged",
        "supplier",
        supplier_id,
        json!({ "exception": ex.id, "task_id": task_id, "month": month,
                "reasons": ex.params.get("reasons") }),
    )
    .await
}

impl Agent for SupplierWatch {
    fn name(&self) -> &'static str {
        NAME
    }

    fn run<'a>(
        &'a self,
        cx: &'a AgentCx,
        run: &'a AgentRun,
    ) -> BoxFuture<'a, Result<Outcome, AgentError>> {
        Box::pin(sweep(cx, run))
    }

    fn apply<'a>(
        &'a self,
        tx: &'a mut Tx,
        ex: &'a AgentException,
        action: &'a Value,
        actor: &'a Actor<'a>,
    ) -> BoxFuture<'a, Result<(), ApiError>> {
        Box::pin(async move {
            match action.get("type").and_then(Value::as_str) {
                Some("request_document_update") => {
                    request_document_update(tx, ex, action, actor).await
                }
                Some("flag_supplier_review") => flag_supplier_review(tx, ex, action, actor).await,
                _ => Err(ApiError::Validation("unknown_action")),
            }
        })
    }

    fn schedule(&self) -> Option<Duration> {
        Some(Duration::from_secs(DAY_SECONDS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_numbers_and_months() {
        let d = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        assert_eq!(to_day(d), compliance::days_from_civil(2026, 11, 5));
        assert_eq!(from_day(to_day(d)), Some(d));
        assert_eq!(
            month_start(d),
            NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()
        );
        assert_eq!(
            next_month(NaiveDate::from_ymd_opt(2026, 12, 1).unwrap()),
            NaiveDate::from_ymd_opt(2027, 1, 1).unwrap()
        );
        assert_eq!(
            prev_month(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
            NaiveDate::from_ymd_opt(2025, 12, 1).unwrap()
        );
        assert_eq!(parse_month("2026-10"), NaiveDate::from_ymd_opt(2026, 10, 1));
        assert_eq!(parse_month("2026-13"), None);
        assert_eq!(parse_month("2026-1"), None);
        assert_eq!(parse_month("26-01"), None);
    }

    #[test]
    fn requests_are_written_in_japanese_or_english() {
        let d = NaiveDate::from_ymd_opt(2026, 12, 1).unwrap();
        let (s, b) =
            document_request_text(true, "東京ベアリング", DocKind::Iso9001, "", d, false, "");
        assert!(s.contains("ISO 9001"), "{s}");
        assert!(b.starts_with("東京ベアリング 御中"), "{b}");
        assert!(b.contains("2026年12月01日に有効期限を迎えます"), "{b}");
        let (s, b) =
            document_request_text(false, "Acme", DocKind::Nda, "", d, true, "Scan is fine.");
        assert_eq!(s, "Request for an updated non-disclosure agreement");
        assert!(b.contains("expired on 2026-12-01"), "{b}");
        assert!(b.contains("Scan is fine."), "{b}");
    }
}
