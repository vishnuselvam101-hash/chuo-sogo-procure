//! The matching agent (L2, docs/agents/DESIGN.md). `orders::record_invoice` queues it when an
//! invoice does not match; a manual run without subject sweeps every disputed invoice.
//! For each open discrepancy it applies the organisation's tolerances
//! (`procure_domain::matching::classify`) and proposes one of:
//! - within tolerance → `resolve_discrepancy` (resolution `accepted`);
//! - out of tolerance and overbilled → `request_credit_note` (a message to the supplier);
//! - anything else → a notice for a person to review, without an action.
//!
//! Nothing changes until a person with ManageInvoices approves; the action then goes through
//! the same resolve logic as the resolve endpoint (`orders::resolve_in_tx`), or writes the
//! credit-note request to the outbox (test mode unless live supplier mail is configured on the
//! server AND the organisation allows agent mail).

use chrono::NaiveDate;
use futures_util::future::BoxFuture;
use procure_domain::agents::{Severity, Trigger};
use procure_domain::matching::{self, Classification, Discrepancy, Tolerance};
use procure_domain::money::{Currency, Money};
use procure_domain::rbac::Permission;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{Actor, Agent, AgentCx, AgentError, AgentException, AgentRun, NewException, Outcome};
use crate::api::common::{self, Audit};
use crate::db::Tx;
use crate::error::{ApiError, ApiResult};

pub const NAME: &str = "matching";
/// At most this many invoices per sweep.
const SWEEP_LIMIT: i64 = 200;

pub struct Matching;

/// The agent exception key of a discrepancy (one open proposal per discrepancy).
pub fn discrepancy_key(discrepancy_id: Uuid) -> String {
    format!("{NAME}:discrepancy:{discrepancy_id}")
}

/// Queues the matching run for a disputed invoice inside the recording transaction (does
/// nothing when agents are off). One waiting run per invoice.
pub async fn enqueue_for_invoice(
    tx: &mut Tx,
    org_id: Uuid,
    invoice_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    super::enqueue(
        tx,
        org_id,
        NAME,
        Trigger::Event,
        Some(("invoice", invoice_id)),
        json!({ "invoice_id": invoice_id }),
        Some(&format!("{NAME}:invoice:{invoice_id}")),
    )
    .await
}

/// The organisation's tolerances as stored (no row = exact, in yen).
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct ToleranceRow {
    pub currency: String,
    pub price_bps: i32,
    pub price_line_abs_minor: i64,
    pub quantity_bps: i32,
    pub quantity_abs: i32,
    pub rounding_minor: i64,
}

impl ToleranceRow {
    pub fn exact() -> Self {
        ToleranceRow {
            currency: "JPY".into(),
            price_bps: 0,
            price_line_abs_minor: 0,
            quantity_bps: 0,
            quantity_abs: 0,
            rounding_minor: 0,
        }
    }

    pub fn to_domain(&self) -> ApiResult<Tolerance> {
        let currency = common::currency(&self.currency)?;
        let u = |v: i32| {
            u32::try_from(v)
                .map_err(|_| ApiError::internal(std::io::Error::other("negative tolerance")))
        };
        Ok(Tolerance {
            currency,
            price_bps: u(self.price_bps)?,
            price_line_abs: Money::from_minor(self.price_line_abs_minor, currency),
            quantity_bps: u(self.quantity_bps)?,
            quantity_abs: u(self.quantity_abs)?,
            rounding: Money::from_minor(self.rounding_minor, currency),
        })
    }
}

pub(crate) async fn load_tolerance(tx: &mut Tx, org_id: Uuid) -> Result<ToleranceRow, sqlx::Error> {
    Ok(sqlx::query_as::<_, ToleranceRow>(
        "SELECT currency, price_bps, price_line_abs_minor, quantity_bps, quantity_abs,
                rounding_minor
         FROM match_tolerances WHERE org_id = $1",
    )
    .bind(org_id)
    .fetch_optional(&mut **tx)
    .await?
    .unwrap_or_else(ToleranceRow::exact))
}

#[derive(sqlx::FromRow)]
struct OpenDiscrepancy {
    id: Uuid,
    invoice_id: Uuid,
    invoice_number: String,
    currency: String,
    request_id: Uuid,
    kind: String,
    expected: Option<i64>,
    actual: Option<i64>,
    item: Option<String>,
    invoiced_quantity: i64,
}

fn finding(d: &OpenDiscrepancy, tol: &Tolerance) -> NewException {
    let currency = Currency::from_code(&d.currency);
    let parsed =
        currency.and_then(|c| Discrepancy::from_stored(&d.kind, (), d.expected, d.actual, c));
    let qty = u32::try_from(d.invoiced_quantity).unwrap_or(0);
    let class = parsed
        .as_ref()
        .map_or(Classification::Review, |x| matching::classify(x, qty, tol));
    let difference = parsed
        .as_ref()
        .and_then(|x| matching::difference(x, qty))
        .map(|m| m.minor());
    let (severity, proposed) = match class {
        Classification::WithinTolerance => (
            Severity::Info,
            Some(
                json!({ "type": "resolve_discrepancy", "discrepancy_id": d.id,
                         "resolution": "accepted" }),
            ),
        ),
        Classification::CreditNote => (
            Severity::Warning,
            Some(json!({ "type": "request_credit_note", "discrepancy_id": d.id })),
        ),
        Classification::Review => (Severity::Warning, None),
    };
    let mut ex = NewException::new(
        class.code(),
        severity,
        &format!("{NAME}.{}", class.code()),
        &discrepancy_key(d.id),
    );
    ex.subject_type = Some("discrepancy".into());
    ex.subject_id = Some(d.id);
    ex.request_id = Some(d.request_id);
    ex.params = json!({
        "invoice_id": d.invoice_id,
        "invoice_number": d.invoice_number,
        "discrepancy_id": d.id,
        "discrepancy_kind": d.kind,
        "item": d.item,
        "expected": d.expected,
        "actual": d.actual,
        "currency": d.currency,
        "invoiced_quantity": d.invoiced_quantity,
        "difference_minor": difference,
    });
    ex.proposed_action = proposed;
    ex
}

impl Agent for Matching {
    fn name(&self) -> &'static str {
        NAME
    }

    fn run<'a>(
        &'a self,
        cx: &'a AgentCx,
        run: &'a AgentRun,
    ) -> BoxFuture<'a, Result<Outcome, AgentError>> {
        Box::pin(async move {
            let invoice = match (run.subject_type.as_deref(), run.subject_id) {
                (Some("invoice"), Some(id)) => Some(id),
                (None, None) => match run.input.get("invoice_id") {
                    None => None,
                    Some(v) => Some(
                        v.as_str()
                            .and_then(|s| Uuid::parse_str(s).ok())
                            .ok_or(AgentError::Fatal("invalid_input"))?,
                    ),
                },
                _ => return Err(AgentError::Fatal("invalid_subject")),
            };
            let mut tx = cx.tx().await?;
            let row = load_tolerance(&mut tx, cx.org_id).await?;
            let tol = row
                .to_domain()
                .map_err(|_| AgentError::Fatal("invalid_tolerance"))?;
            // Open discrepancies of disputed invoices that have never had a proposal decided
            // (a rejected proposal is not raised again by a sweep).
            let rows = sqlx::query_as::<_, OpenDiscrepancy>(
                "SELECT d.id, i.id AS invoice_id, i.supplier_invoice_number AS invoice_number,
                        i.currency, o.request_id, d.kind, d.expected, d.actual,
                        CASE WHEN ri.id IS NULL THEN NULL
                             ELSE coalesce(nullif(btrim(coalesce(ri.verified_mpn, ri.mpn)), ''),
                                           ri.description) END AS item,
                        coalesce((SELECT sum(l.quantity) FROM invoice_lines l
                                  WHERE l.invoice_id = i.id
                                    AND l.order_item_id = d.order_item_id), 0)::bigint
                            AS invoiced_quantity
                 FROM discrepancies d
                 JOIN invoices i ON i.id = d.invoice_id
                 JOIN purchase_orders o ON o.id = i.order_id
                 LEFT JOIN order_items oi ON oi.id = d.order_item_id
                 LEFT JOIN request_items ri ON ri.id = oi.request_item_id
                 WHERE d.status = 'open' AND i.match_status = 'disputed'
                   AND ($1::uuid IS NULL OR i.id = $1)
                   AND i.id IN (SELECT id FROM invoices WHERE match_status = 'disputed'
                                  AND ($1::uuid IS NULL OR id = $1)
                                ORDER BY created_at LIMIT $2)
                   AND NOT EXISTS (SELECT 1 FROM agent_exceptions e
                                   WHERE e.dedupe_key = $3 || d.id::text
                                     AND e.status <> 'open')
                 ORDER BY i.created_at, d.id",
            )
            .bind(invoice)
            .bind(SWEEP_LIMIT)
            .bind(format!("{NAME}:discrepancy:"))
            .fetch_all(&mut *tx)
            .await?;
            tx.commit().await?;
            let exceptions: Vec<NewException> = rows.iter().map(|d| finding(d, &tol)).collect();
            let count = |k: &str| exceptions.iter().filter(|e| e.kind == k).count();
            let output = json!({
                "discrepancies": exceptions.len(),
                "within_tolerance": count("within_tolerance"),
                "out_of_tolerance": count("out_of_tolerance"),
                "needs_review": count("needs_review"),
            });
            Ok(Outcome {
                output,
                exceptions,
                model_calls: 0,
            })
        })
    }

    fn apply<'a>(
        &'a self,
        tx: &'a mut Tx,
        ex: &'a AgentException,
        action: &'a Value,
        actor: &'a Actor<'a>,
    ) -> BoxFuture<'a, Result<(), ApiError>> {
        Box::pin(async move {
            // An edit may change details, never which discrepancy the action is about.
            let target = action
                .get("discrepancy_id")
                .and_then(Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok());
            let id = match (target, ex.subject_id) {
                (Some(t), Some(s))
                    if t == s && ex.subject_type.as_deref() == Some("discrepancy") =>
                {
                    t
                }
                _ => return Err(ApiError::Validation("action")),
            };
            let audit = Audit {
                org_id: ex.org_id,
                p: actor.principal,
            };
            match action.get("type").and_then(Value::as_str) {
                Some("resolve_discrepancy") => {
                    actor.require(Permission::ManageInvoices)?;
                    let side = common::side(actor.access)?;
                    let resolution = action
                        .get("resolution")
                        .and_then(Value::as_str)
                        .unwrap_or("accepted");
                    let note = if ex.decision_note.trim().is_empty() {
                        format!("agent:{NAME}:{}", ex.id)
                    } else {
                        ex.decision_note.trim().to_string()
                    };
                    crate::api::orders::resolve_in_tx(tx, &audit, side, id, resolution, &note).await
                }
                Some("request_credit_note") => {
                    actor.require(Permission::ManageInvoices)?;
                    request_credit_note(tx, &audit, actor, ex, id).await
                }
                _ => Err(ApiError::Validation("unknown_action")),
            }
        })
    }
}

#[derive(sqlx::FromRow)]
struct CreditNoteRow {
    status: String,
    kind: String,
    expected: Option<i64>,
    actual: Option<i64>,
    invoice_id: Uuid,
    invoice_number: String,
    invoice_date: NaiveDate,
    currency: String,
    po_number: String,
    factory_name: String,
    recipient_id: Uuid,
    supplier_name: String,
    email: Option<String>,
    fax: Option<String>,
    rfq_channel: String,
    item: Option<String>,
    invoiced_quantity: i64,
}

/// Supplier-facing request text (Japanese first, English below), from fixed server-side
/// templates; only our own records fill the placeholders.
mod text {
    pub const SUBJECT: &str = "請求書訂正のお願い / Credit note request: invoice {invoice}";
    pub const BODY_JA: &str = "{supplier} 御中\n\n\
        いつもお世話になっております。\n\
        {factory} 様の注文書 {po} に対する貴社ご請求書 {invoice}（{date} 付）を照合いたしましたところ、\
        下記の差異がございました。\n\n{detail}\n\n\
        恐れ入りますが、差額分の赤伝（クレジットノート）または訂正後のご請求書をご発行いただけますでしょうか。\n\
        ご返信の際は件名を変更せずにお送りください。\n";
    pub const BODY_EN: &str = "Dear {supplier},\n\n\
        We checked your invoice {invoice} dated {date} for purchase order {po} ({factory}) \
        and found the difference below.\n\n{detail}\n\n\
        Please issue a credit note for the difference, or a corrected invoice. Please reply \
        without changing the subject.\n";
}

fn fill(template: &str, values: &[(&str, &str)]) -> String {
    values.iter().fold(template.to_string(), |t, (k, v)| {
        t.replace(&format!("{{{k}}}"), v)
    })
}

/// (Japanese, English) description of the difference.
fn detail(r: &CreditNoteRow) -> ApiResult<(String, String)> {
    let currency = common::currency(&r.currency)?;
    let money = |v: Option<i64>| {
        v.map(|m| crate::api::savings::amount_text(Money::from_minor(m, currency)))
            .unwrap_or_default()
    };
    let num = |v: Option<i64>| v.map(|n| n.to_string()).unwrap_or_default();
    let item = r
        .item
        .as_deref()
        .unwrap_or("")
        .replace(['\n', '\r'], " ")
        .chars()
        .take(200)
        .collect::<String>();
    let (e, a) = (r.expected, r.actual);
    Ok(match r.kind.as_str() {
        "price_mismatch" => {
            let diff = match (e, a) {
                (Some(e), Some(a)) => (a - e).checked_mul(r.invoiced_quantity),
                _ => None,
            };
            (
                format!(
                    "品目 {item}：ご注文単価 {} に対し、ご請求単価 {}（数量 {}、差額 {}）",
                    money(e),
                    money(a),
                    r.invoiced_quantity,
                    money(diff)
                ),
                format!(
                    "Item {item}: ordered at {} per unit, invoiced at {} ({} units, difference {})",
                    money(e),
                    money(a),
                    r.invoiced_quantity,
                    money(diff)
                ),
            )
        }
        "quantity_over_ordered" => (
            format!(
                "品目 {item}：ご注文数量 {} に対し、ご請求数量 {}",
                num(e),
                num(a)
            ),
            format!("Item {item}: ordered {}, invoiced {}", num(e), num(a)),
        ),
        "quantity_over_received" => (
            format!(
                "品目 {item}：受領数量 {} に対し、ご請求数量 {}",
                num(e),
                num(a)
            ),
            format!("Item {item}: received {}, invoiced {}", num(e), num(a)),
        ),
        "shipping_mismatch" => (
            format!("送料：ご注文時 {} に対し、ご請求 {}", money(e), money(a)),
            format!("Shipping: ordered at {}, invoiced {}", money(e), money(a)),
        ),
        "tax_mismatch" => (
            format!(
                "消費税額：再計算額 {} に対し、ご請求 {}",
                money(e),
                money(a)
            ),
            format!("Tax: recomputed {}, invoiced {}", money(e), money(a)),
        ),
        "unknown_item" => (
            "ご注文にない品目がご請求に含まれています。".to_string(),
            "The invoice bills an item that is not on the order.".to_string(),
        ),
        "duplicate_line" => (
            format!("品目 {item}：同じ品目が複数行でご請求されています。"),
            format!("Item {item}: billed on more than one line."),
        ),
        // Only overbilling is proposed as a credit note; anything else is refused.
        _ => return Err(ApiError::Validation("not_a_credit_note_case")),
    })
}

async fn request_credit_note(
    tx: &mut Tx,
    audit: &Audit<'_>,
    actor: &Actor<'_>,
    ex: &AgentException,
    id: Uuid,
) -> ApiResult<()> {
    let r = sqlx::query_as::<_, CreditNoteRow>(
        "SELECT d.status, d.kind, d.expected, d.actual, i.id AS invoice_id,
                i.supplier_invoice_number AS invoice_number, i.invoice_date, i.currency,
                o.po_number, g.name AS factory_name, x.id AS recipient_id,
                s.name AS supplier_name, s.email::text AS email, s.fax, s.rfq_channel,
                CASE WHEN ri.id IS NULL THEN NULL
                     ELSE coalesce(nullif(btrim(coalesce(ri.verified_mpn, ri.mpn)), ''),
                                   ri.description) END AS item,
                coalesce((SELECT sum(l.quantity) FROM invoice_lines l
                          WHERE l.invoice_id = i.id AND l.order_item_id = d.order_item_id), 0)::bigint
                    AS invoiced_quantity
         FROM discrepancies d
         JOIN invoices i ON i.id = d.invoice_id
         JOIN purchase_orders o ON o.id = i.order_id
         JOIN organisations g ON g.id = o.org_id
         JOIN recommendations rc ON rc.id = o.recommendation_id
         JOIN quotations q ON q.id = rc.quotation_id
         JOIN rfq_recipients x ON x.id = q.rfq_recipient_id
         JOIN suppliers s ON s.id = x.supplier_id
         LEFT JOIN order_items oi ON oi.id = d.order_item_id
         LEFT JOIN request_items ri ON ri.id = oi.request_item_id
         WHERE d.id = $1
         FOR UPDATE OF d",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    if r.status != "open" {
        return Err(ApiError::Conflict("already_resolved"));
    }
    let (detail_ja, detail_en) = detail(&r)?;
    let date = r.invoice_date.to_string();
    let values = |detail: &str| -> Vec<(&str, String)> {
        vec![
            ("supplier", r.supplier_name.clone()),
            ("factory", r.factory_name.clone()),
            ("po", r.po_number.clone()),
            ("invoice", r.invoice_number.clone()),
            ("date", date.clone()),
            ("detail", detail.to_string()),
        ]
    };
    let render = |template: &str, detail: &str| {
        let v = values(detail);
        let pairs: Vec<(&str, &str)> = v.iter().map(|(k, s)| (*k, s.as_str())).collect();
        fill(template, &pairs)
    };
    let body = format!(
        "{}\n----------------------------------------\n\n{}",
        render(text::BODY_JA, &detail_ja),
        render(text::BODY_EN, &detail_en)
    );
    // Test mode unless the server has live supplier mail AND this organisation allows mail
    // from approved agent actions.
    let auto_send: bool = sqlx::query_scalar(
        "SELECT coalesce((SELECT auto_send_supplier_mail FROM agent_settings WHERE org_id = $1),
                         false)",
    )
    .bind(ex.org_id)
    .fetch_one(&mut **tx)
    .await?;
    let (channel, to) = match (&actor.state.supplier_mail, auto_send) {
        (Some(m), true) => {
            let (c, to) = m
                .address(&r.rfq_channel, r.email.as_deref(), r.fax.as_deref())
                .map_err(ApiError::Conflict)?;
            (c, Some(to))
        }
        _ => ("test", r.email.clone()),
    };
    let token = crate::supplier_mail::reply_token();
    let subject = format!(
        "{} [#{token}]",
        fill(text::SUBJECT, &[("invoice", r.invoice_number.as_str())])
    )
    .chars()
    .take(300)
    .collect::<String>();
    let outbox_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO outbox (id, org_id, rfq_recipient_id, channel, to_address, subject, body,
                             kind, reply_token, status, next_attempt_at, checked_by,
                             discrepancy_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'credit_note_request', $8,
                 CASE WHEN $4 = 'test' THEN 'recorded' ELSE 'queued' END,
                 CASE WHEN $4 = 'test' THEN NULL ELSE now() END, $9, $10)",
    )
    .bind(outbox_id)
    .bind(ex.org_id)
    .bind(r.recipient_id)
    .bind(channel)
    .bind(to)
    .bind(&subject)
    .bind(&body)
    .bind(&token)
    .bind(actor.principal.user_id)
    .bind(id)
    .execute(&mut **tx)
    .await
    .map_err(|e| match ApiError::from(e) {
        ApiError::Conflict(_) => ApiError::Conflict("credit_note_already_requested"),
        other => other,
    })?;
    audit
        .record(
            tx,
            "matching.credit_note_requested",
            "discrepancy",
            id,
            json!({ "invoice": r.invoice_id, "kind": r.kind, "outbox": outbox_id,
                    "channel": channel, "supplier": r.supplier_name,
                    "agent_exception": ex.id }),
        )
        .await
}
