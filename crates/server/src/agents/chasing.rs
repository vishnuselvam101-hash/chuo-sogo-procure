//! The chasing agent (lane L3): follows up with suppliers on open orders.
//!
//! - **Sweep** (scheduled): finds orders whose order email was not acknowledged, that have
//!   no delivery date, whose expected delivery date passed without everything arriving, or
//!   that arrived but were not invoiced, after the organisation's cadence (Japan business
//!   days, `procure_domain::chasing`). Each finding is an exception proposing
//!   `send_chase` with a polite draft (Japanese for suppliers in Japan, English otherwise;
//!   it names the order numbers, never the customer). One exception per order, type and
//!   attempt, ever; after the last attempt a person is asked to take over.
//! - **Approve** `send_chase`: the message goes into the outbox like every supplier message,
//!   test mode (recorded) unless live mail is configured **and** the organisation set
//!   `auto_send_supplier_mail`. With both, later reminders (attempt 2+) are queued directly
//!   by the sweep, without an exception: the first follow-up was approved by a person.
//! - **Reply**: a supplier message filed for an order email or follow-up is read by the
//!   `parse_supplier_reply` capability into an `ai_suggestions` row and an exception
//!   proposing `record_promised_date` or `confirm_delivery_date`, applied through the same
//!   rules as the staff route. Replies that try to give instructions are flagged and
//!   propose nothing.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use futures_util::future::BoxFuture;
use procure_ai::reply::{self, Ack, ReplyContext, SupplierReply};
use procure_domain::agents::{Severity, Trigger};
use procure_domain::chasing::{
    self as rules, Cadence, ChaseType, Draft, DraftContext, Due, History, Lang, OrderFacts,
};
use procure_domain::rbac::Permission;
use procure_domain::workflow::{RequestStatus, Side};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{Actor, Agent, AgentCx, AgentError, AgentException, AgentRun, NewException, Outcome};
use crate::AppState;
use crate::api::common::{self, Audit, View, lock_request};
use crate::audit::{self, Event};
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiResult};

pub const NAME: &str = "chasing";
/// How often the sweep runs per organisation.
pub const SWEEP_EVERY: Duration = Duration::from_secs(3600);
/// Orders looked at per sweep.
const SWEEP_LIMIT: i64 = 500;
const MAX_SUBJECT: usize = 250;
const MAX_BODY: usize = 20_000;

pub struct Chasing;

// ---------------------------------------------------------------------------------------
// Settings

/// The organisation's cadence (defaults when never set).
pub async fn cadence(tx: &mut Tx, org_id: Uuid) -> Result<Cadence, sqlx::Error> {
    let row: Option<(i32, i32, i32, i32, i32, i32)> = sqlx::query_as(
        "SELECT ack_days, delivery_date_days, overdue_days, invoice_days, repeat_days,
                max_attempts
         FROM chase_settings WHERE org_id = $1",
    )
    .bind(org_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map_or_else(Cadence::default, |r| Cadence {
        ack_days: r.0,
        delivery_date_days: r.1,
        overdue_days: r.2,
        invoice_days: r.3,
        repeat_days: r.4,
        max_attempts: r.5,
    }))
}

/// Automatic supplier mail is two-party: the customer's admin requested it and Chuo-Sogo
/// staff confirmed that request.
async fn auto_send(tx: &mut Tx, org_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT coalesce((SELECT auto_send_supplier_mail AND auto_send_confirmed_at IS NOT NULL
                          FROM agent_settings WHERE org_id = $1), false)",
    )
    .bind(org_id)
    .fetch_one(&mut **tx)
    .await
}

/// Agent mail leaves the server only when live mail is configured and the organisation
/// allows it; otherwise it is recorded (test mode).
async fn live_mail(state: &AppState, tx: &mut Tx, org_id: Uuid) -> Result<bool, sqlx::Error> {
    Ok(state.supplier_mail.is_some() && auto_send(tx, org_id).await?)
}

// ---------------------------------------------------------------------------------------
// Orders

#[derive(Debug, Clone, sqlx::FromRow)]
struct OrderRow {
    id: Uuid,
    request_id: Uuid,
    po_number: String,
    status: String,
    promised_on: Option<NaiveDate>,
    delivery_confirmed_on: Option<NaiveDate>,
    acknowledged_at: Option<DateTime<Utc>>,
    order_sent_at: Option<DateTime<Utc>>,
    last_received_at: Option<DateTime<Utc>>,
    invoiced: bool,
    supplier_id: Uuid,
    supplier_name: String,
    supplier_country: String,
    rfq_number: String,
    recipient_id: Uuid,
}

impl OrderRow {
    fn facts(&self) -> OrderFacts {
        OrderFacts {
            order_sent_at: self.order_sent_at,
            acknowledged: self.acknowledged_at.is_some() || self.delivery_confirmed_on.is_some(),
            expected_on: self.delivery_confirmed_on.or(self.promised_on),
            awaiting_delivery: self.status == "ordered",
            received_complete_at: (self.status == "delivered")
                .then_some(self.last_received_at)
                .flatten(),
            invoiced: self.invoiced,
        }
    }

    fn draft(&self, t: ChaseType, attempt: i32) -> Draft {
        rules::draft(
            t,
            attempt,
            Lang::for_country(&self.supplier_country),
            &DraftContext {
                supplier_name: &self.supplier_name,
                po_number: &self.po_number,
                rfq_number: &self.rfq_number,
                expected_on: self.delivery_confirmed_on.or(self.promised_on),
            },
        )
    }
}

/// Open orders (`ordered` or `delivered`) of the organisation in `tx`'s context, or the one
/// with `id`.
async fn open_orders(tx: &mut Tx, id: Option<Uuid>) -> Result<Vec<OrderRow>, sqlx::Error> {
    sqlx::query_as::<_, OrderRow>(
        "SELECT o.id, o.request_id, o.po_number, r.status, o.promised_on,
                o.delivery_confirmed_on, o.acknowledged_at,
                (SELECT min(b.created_at) FROM outbox b
                 WHERE b.order_id = o.id AND b.kind = 'order') AS order_sent_at,
                (SELECT max(d.received_at) FROM deliveries d
                 WHERE d.order_id = o.id AND d.reversed_at IS NULL) AS last_received_at,
                EXISTS (SELECT 1 FROM invoices i WHERE i.order_id = o.id) AS invoiced,
                s.id AS supplier_id, s.name AS supplier_name, s.country AS supplier_country,
                f.number AS rfq_number, x.id AS recipient_id
         FROM purchase_orders o
         JOIN procurement_requests r ON r.id = o.request_id
         JOIN recommendations rc ON rc.id = o.recommendation_id
         JOIN quotations q ON q.id = rc.quotation_id
         JOIN rfq_recipients x ON x.id = q.rfq_recipient_id
         JOIN rfqs f ON f.id = x.rfq_id
         JOIN suppliers s ON s.id = x.supplier_id
         WHERE r.status IN ('ordered', 'delivered') AND ($1::uuid IS NULL OR o.id = $1)
         ORDER BY o.ordered_at
         LIMIT $2",
    )
    .bind(id)
    .bind(SWEEP_LIMIT)
    .fetch_all(&mut **tx)
    .await
}

// ---------------------------------------------------------------------------------------
// Sweep

fn chase_key(order: Uuid, t: ChaseType, attempt: i32) -> String {
    format!("chasing:chase:{order}:{}:{attempt}", t.code())
}

fn exhausted_key(order: Uuid, t: ChaseType) -> String {
    format!("chasing:exhausted:{order}:{}", t.code())
}

/// What the sweep found for one order.
#[derive(Debug, Clone)]
pub struct Finding {
    pub order_id: Uuid,
    pub chase_type: ChaseType,
    pub due: Due,
    /// A reminder (attempt 2+) that may go out without a new decision: the previous attempt
    /// is in the outbox and a person approved and sent a follow-up of this series.
    pub may_auto_send: bool,
}

struct Known {
    history: HashMap<(Uuid, ChaseType), History>,
    /// Orders with a reply waiting for a person: no follow-up meanwhile.
    replied: HashSet<Uuid>,
    exhausted: HashSet<String>,
    /// Series (order, type) a person rejected a follow-up of: no further follow-ups.
    stopped: HashSet<(Uuid, ChaseType)>,
    /// Follow-ups in the outbox (order, type, attempt).
    sent: HashSet<(Uuid, ChaseType, i32)>,
    /// Series with a follow-up a person approved (`checked_by` set) and sent.
    approved: HashSet<(Uuid, ChaseType)>,
}

impl Known {
    fn may_auto_send(&self, order: Uuid, t: ChaseType, attempt: i32) -> bool {
        attempt >= 2
            && self.approved.contains(&(order, t))
            && self.sent.contains(&(order, t, attempt - 1))
    }
}

fn note(h: &mut History, attempt: i32, at: DateTime<Utc>) {
    if attempt > h.attempts {
        h.attempts = attempt;
    }
    if h.last_at.is_none_or(|l| at > l) {
        h.last_at = Some(at);
    }
}

/// (order, chase type, attempt, status, decided or raised at)
type ProposedRow = (
    Option<Uuid>,
    Option<String>,
    Option<i32>,
    String,
    DateTime<Utc>,
);

async fn known(tx: &mut Tx) -> Result<Known, sqlx::Error> {
    let mut history: HashMap<(Uuid, ChaseType), History> = HashMap::new();
    let mut sent = HashSet::new();
    let mut approved = HashSet::new();
    let rows: Vec<(Uuid, String, i32, DateTime<Utc>, bool)> = sqlx::query_as(
        "SELECT order_id, chase_type, chase_attempt, created_at, checked_by IS NOT NULL
         FROM outbox WHERE kind = 'chase'",
    )
    .fetch_all(&mut **tx)
    .await?;
    for (order, t, attempt, at, checked) in rows {
        if let Some(t) = ChaseType::from_code(&t) {
            note(history.entry((order, t)).or_default(), attempt, at);
            sent.insert((order, t, attempt));
            if checked {
                approved.insert((order, t));
            }
        }
    }
    let mut stopped = HashSet::new();
    let proposed: Vec<ProposedRow> = sqlx::query_as(
        "SELECT subject_id, params ->> 'chase_type', (params ->> 'attempt')::int,
                    status, coalesce(decided_at, created_at)
             FROM agent_exceptions WHERE agent = 'chasing' AND kind = 'chase_due'",
    )
    .fetch_all(&mut **tx)
    .await?;
    for (order, t, attempt, status, at) in proposed {
        let (Some(order), Some(t), Some(attempt)) =
            (order, t.as_deref().and_then(ChaseType::from_code), attempt)
        else {
            continue;
        };
        let h = history.entry((order, t)).or_default();
        match status.as_str() {
            "open" => h.pending = true,
            // A person said no: the series ends here (a person may still write by hand).
            "rejected" => {
                stopped.insert((order, t));
            }
            // Sent, or handled elsewhere: the next attempt waits for the cadence.
            _ => note(h, attempt, at),
        }
    }
    let replied: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT subject_id FROM agent_exceptions
         WHERE agent = 'chasing' AND status = 'open' AND kind LIKE 'reply\\_%'
           AND subject_id IS NOT NULL",
    )
    .fetch_all(&mut **tx)
    .await?;
    let exhausted: Vec<String> = sqlx::query_scalar(
        "SELECT dedupe_key FROM agent_exceptions
         WHERE agent = 'chasing' AND kind = 'chase_exhausted'",
    )
    .fetch_all(&mut **tx)
    .await?;
    Ok(Known {
        history,
        replied: replied.into_iter().collect(),
        exhausted: exhausted.into_iter().collect(),
        stopped,
        sent,
        approved,
    })
}

/// The follow-ups due at `now` for the organisation of `tx` (read only).
pub async fn findings(
    tx: &mut Tx,
    org_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Vec<Finding>, sqlx::Error> {
    let cadence = cadence(tx, org_id).await?;
    let known = known(tx).await?;
    let mut out = Vec::new();
    for o in open_orders(tx, None).await? {
        if known.replied.contains(&o.id) {
            continue;
        }
        let facts = o.facts();
        for t in ChaseType::ALL {
            if known.stopped.contains(&(o.id, t)) {
                continue;
            }
            let h = known.history.get(&(o.id, t)).copied().unwrap_or_default();
            match rules::due(t, &facts, &h, &cadence, now) {
                Due::No => {}
                Due::Exhausted if known.exhausted.contains(&exhausted_key(o.id, t)) => {}
                due => out.push(Finding {
                    order_id: o.id,
                    chase_type: t,
                    due,
                    may_auto_send: match due {
                        Due::Attempt(n) => known.may_auto_send(o.id, t, n),
                        _ => false,
                    },
                }),
            }
        }
    }
    Ok(out)
}

fn draft_json(d: &Draft) -> Value {
    json!({ "lang": d.lang.code(), "subject": d.subject, "body": d.body })
}

fn chase_exception(o: &OrderRow, t: ChaseType, attempt: i32) -> NewException {
    let severity = if attempt > 1 || t == ChaseType::OverdueDelivery {
        Severity::Warning
    } else {
        Severity::Info
    };
    let mut ex = NewException::new(
        "chase_due",
        severity,
        &format!("chasing.due.{}", t.code()),
        &chase_key(o.id, t, attempt),
    );
    ex.subject_type = Some("purchase_order".into());
    ex.subject_id = Some(o.id);
    ex.request_id = Some(o.request_id);
    ex.params = json!({
        "order_id": o.id, "po_number": o.po_number, "supplier_name": o.supplier_name,
        "chase_type": t.code(), "attempt": attempt,
        "expected_on": o.delivery_confirmed_on.or(o.promised_on),
    });
    ex.proposed_action = Some(json!({
        "type": "send_chase", "order_id": o.id, "chase_type": t.code(), "attempt": attempt,
        "draft": draft_json(&o.draft(t, attempt)),
    }));
    ex
}

fn exhausted_exception(o: &OrderRow, t: ChaseType, attempts: i32) -> NewException {
    let mut ex = NewException::new(
        "chase_exhausted",
        Severity::Warning,
        &format!("chasing.exhausted.{}", t.code()),
        &exhausted_key(o.id, t),
    );
    ex.subject_type = Some("purchase_order".into());
    ex.subject_id = Some(o.id);
    ex.request_id = Some(o.request_id);
    ex.params = json!({
        "order_id": o.id, "po_number": o.po_number, "supplier_name": o.supplier_name,
        "chase_type": t.code(), "attempts": attempts,
    });
    ex
}

async fn sweep(cx: &AgentCx, now: DateTime<Utc>) -> Result<Outcome, AgentError> {
    let mut tx = cx.tx().await?;
    let found = findings(&mut tx, cx.org_id, now).await?;
    let orders: HashMap<Uuid, OrderRow> = open_orders(&mut tx, None)
        .await?
        .into_iter()
        .map(|o| (o.id, o))
        .collect();
    let cadence = cadence(&mut tx, cx.org_id).await?;
    let live = live_mail(&cx.state, &mut tx, cx.org_id).await?;
    let mut exceptions = Vec::new();
    let mut auto_sent = Vec::new();
    for f in &found {
        let Some(o) = orders.get(&f.order_id) else {
            continue;
        };
        match f.due {
            Due::No => {}
            Due::Exhausted => {
                exceptions.push(exhausted_exception(o, f.chase_type, cadence.max_attempts))
            }
            // A person approved and sent a follow-up of this series and the previous attempt
            // went out; with live agent mail allowed, the reminders after it go out on the
            // cadence. A rejected follow-up ends the series (no finding at all).
            Due::Attempt(n) if n >= 2 && live && f.may_auto_send => {
                let d = o.draft(f.chase_type, n);
                match queue_chase(
                    &cx.state,
                    &mut tx,
                    cx.org_id,
                    o,
                    f.chase_type,
                    n,
                    &d.subject,
                    &d.body,
                    None,
                )
                .await
                {
                    Ok(channel) => {
                        audit::record(
                            &mut tx,
                            Event {
                                org_id: cx.org_id,
                                actor: None,
                                action: "order.follow_up_sent",
                                entity_type: "purchase_order",
                                entity_id: Some(o.id.to_string()),
                                data: json!({ "chase_type": f.chase_type.code(), "attempt": n,
                                              "channel": channel, "by": "agent",
                                              "run_id": cx.run_id }),
                                request_id: None,
                            },
                        )
                        .await?;
                        auto_sent.push(json!({ "order_id": o.id, "chase_type": f.chase_type.code(), "attempt": n }));
                    }
                    // The supplier cannot be reached by mail: a person decides instead.
                    Err(ApiError::Conflict(code)) => {
                        tracing::info!(order = %o.id, code, "follow-up not sent automatically");
                        exceptions.push(chase_exception(o, f.chase_type, n));
                    }
                    Err(e) => {
                        tracing::warn!(order = %o.id, error = ?e, "follow-up not queued");
                        return Err(AgentError::Retry("outbox"));
                    }
                }
            }
            Due::Attempt(n) => exceptions.push(chase_exception(o, f.chase_type, n)),
        }
    }
    // The worker raises the exceptions and ends the run in this same transaction, so the
    // reminders queued above exist only if the run's outcome is saved too.
    Ok(Outcome {
        output: json!({ "orders": orders.len(), "proposed": exceptions.len(),
                        "auto_sent": auto_sent }),
        exceptions,
        model_calls: 0,
        tx: Some(tx),
    })
}

// ---------------------------------------------------------------------------------------
// Outbox

fn one_line(s: &str, max: usize) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .chars()
        .take(max)
        .collect()
}

/// Writes one follow-up to the outbox (recorded in test mode; queued when agent mail may go
/// out live). `checked_by` is the person who approved it, `None` for an automatic
/// reminder. Returns "test" or "live".
#[allow(clippy::too_many_arguments)]
async fn queue_chase(
    state: &AppState,
    tx: &mut Tx,
    org_id: Uuid,
    o: &OrderRow,
    t: ChaseType,
    attempt: i32,
    subject: &str,
    body: &str,
    checked_by: Option<Uuid>,
) -> ApiResult<&'static str> {
    let (name, email, fax, rfq_channel): (String, Option<String>, Option<String>, String) =
        sqlx::query_as(
            "SELECT s.name, s.email::text, s.fax, s.rfq_channel
             FROM rfq_recipients x JOIN suppliers s ON s.id = x.supplier_id WHERE x.id = $1",
        )
        .bind(o.recipient_id)
        .fetch_one(&mut **tx)
        .await?;
    let live = live_mail(state, tx, org_id).await?;
    let (channel, to) = match (&state.supplier_mail, live) {
        (Some(m), true) => {
            let (c, to) = m
                .address(&rfq_channel, email.as_deref(), fax.as_deref())
                .map_err(|code| {
                    tracing::info!(supplier = %name, code, "recipient cannot be reached");
                    ApiError::Conflict(code)
                })?;
            (c, Some(to))
        }
        _ => ("test", email),
    };
    let token = crate::supplier_mail::reply_token();
    let subject = format!("{} [#{token}]", one_line(subject, MAX_SUBJECT));
    let inserted = sqlx::query(
        "INSERT INTO outbox (id, org_id, rfq_recipient_id, channel, to_address, subject, body,
                             kind, reply_token, status, next_attempt_at, order_id, checked_by,
                             chase_type, chase_attempt)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'chase', $8,
                 CASE WHEN $4 = 'test' THEN 'recorded' ELSE 'queued' END,
                 CASE WHEN $4 = 'test' THEN NULL ELSE now() END, $9, $10, $11, $12)
         ON CONFLICT (order_id, chase_type, chase_attempt) WHERE kind = 'chase' DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(org_id)
    .bind(o.recipient_id)
    .bind(channel)
    .bind(to)
    .bind(&subject)
    .bind(body)
    .bind(&token)
    .bind(o.id)
    .bind(checked_by)
    .bind(t.code())
    .bind(attempt)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if inserted == 0 {
        return Err(ApiError::Conflict("already_sent"));
    }
    Ok(if channel == "test" { "test" } else { "live" })
}

// ---------------------------------------------------------------------------------------
// Replies

/// Called when a supplier message is filed on an RFQ (mailbox poll or staff assignment), in
/// that transaction: a reply quoting the token of an order email or follow-up queues the
/// agent to read it (nothing when agents are off).
pub(crate) async fn on_reply_kept(
    tx: &mut Tx,
    org_id: Uuid,
    document_id: Uuid,
    bytes: &[u8],
) -> Result<(), sqlx::Error> {
    let Some(subject) = mail_parser::MessageParser::default()
        .parse(bytes)
        .and_then(|m| m.subject().map(str::to_owned))
    else {
        return Ok(());
    };
    let Some(token) = crate::supplier_mail::token_in(&subject) else {
        return Ok(());
    };
    let hit: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, order_id FROM outbox
         WHERE reply_token = $1 AND kind IN ('order', 'chase') AND order_id IS NOT NULL",
    )
    .bind(&token)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((outbox_id, order_id)) = hit else {
        return Ok(());
    };
    super::enqueue(
        tx,
        org_id,
        NAME,
        Trigger::Event,
        Some(("supplier_document", document_id)),
        json!({ "document_id": document_id, "order_id": order_id, "outbox_id": outbox_id }),
        Some(&format!("chasing:reply:{document_id}")),
    )
    .await
    .map(|_| ())
}

/// Checks a manual request to read a reply (`input: {document_id, order_id}`, transaction
/// in the organisation's context): both exist in this organisation, the order is open and
/// the document came from the order's supplier.
pub(crate) async fn check_reply_input(tx: &mut Tx, input: &Value) -> ApiResult<()> {
    let id = |k: &'static str| {
        input
            .get(k)
            .and_then(Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or(ApiError::Validation(k))
    };
    let (document_id, order_id) = (id("document_id")?, id("order_id")?);
    let supplier: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT supplier_id FROM supplier_documents WHERE id = $1")
            .bind(document_id)
            .fetch_optional(&mut **tx)
            .await?;
    let Some(supplier) = supplier else {
        return Err(ApiError::Validation("document_id"));
    };
    let order = open_orders(tx, Some(order_id))
        .await?
        .pop()
        .ok_or(ApiError::Validation("order_id"))?;
    if supplier != Some(order.supplier_id) {
        return Err(ApiError::Validation("document_id"));
    }
    Ok(())
}

fn reply_exception(
    o: &OrderRow,
    document_id: Uuid,
    r: &SupplierReply,
    suggestion: Uuid,
) -> NewException {
    let (kind, severity, action) = if r.suspicious() {
        // Inert: nothing is proposed from a reply that tries to give instructions.
        ("reply_suspicious", Severity::Critical, None)
    } else if r.acknowledged != Ack::Yes {
        ("reply_attention", Severity::Warning, None)
    } else {
        let action = match r.promised_date {
            Some(d) if o.promised_on.is_none() && o.delivery_confirmed_on.is_none() => json!({
                "type": "record_promised_date", "order_id": o.id, "date": d,
            }),
            Some(d) => json!({
                "type": "confirm_delivery_date", "order_id": o.id, "date": d,
                "note": "supplier reply",
            }),
            None => json!({ "type": "record_promised_date", "order_id": o.id, "date": null }),
        };
        let kind = if r.promised_date.is_some() {
            "reply_confirmed"
        } else {
            "reply_acknowledged"
        };
        let severity = if r.needs_review {
            Severity::Warning
        } else {
            Severity::Info
        };
        (kind, severity, Some(action))
    };
    let mut ex = NewException::new(
        kind,
        severity,
        &format!("chasing.reply.{}", kind.trim_start_matches("reply_")),
        &format!("chasing:reply:{document_id}"),
    );
    ex.subject_type = Some("purchase_order".into());
    ex.subject_id = Some(o.id);
    ex.request_id = Some(o.request_id);
    ex.suggestion_id = Some(suggestion);
    ex.params = json!({
        "order_id": o.id, "po_number": o.po_number, "supplier_name": o.supplier_name,
        "document_id": document_id,
        "acknowledged": r.acknowledged, "promised_date": r.promised_date,
        "partial_quantity": r.partial_quantity, "questions": r.questions,
        "flags": r.flags, "confidence": r.confidence,
        "expected_on": o.delivery_confirmed_on.or(o.promised_on),
    });
    ex.proposed_action = action;
    ex
}

async fn read_reply(cx: &AgentCx, run: &AgentRun) -> Result<Outcome, AgentError> {
    let uuid_of = |k: &str| {
        run.input[k]
            .as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or(AgentError::Fatal("invalid_input"))
    };
    let (document_id, order_id) = (uuid_of("document_id")?, uuid_of("order_id")?);
    let mut tx = cx.tx().await?;
    let doc: Option<(Vec<u8>, Option<Uuid>)> =
        sqlx::query_as("SELECT sha256, supplier_id FROM supplier_documents WHERE id = $1")
            .bind(document_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((sha, supplier_id)) = doc else {
        return Err(AgentError::Fatal("document_not_found"));
    };
    let Some(order) = open_orders(&mut tx, Some(order_id)).await?.pop() else {
        tx.commit().await?;
        return Ok(Outcome::new(json!({ "skipped": "order_not_open" })));
    };
    // Only the order's own supplier can answer for it.
    if supplier_id != Some(order.supplier_id) {
        return Err(AgentError::Fatal("supplier_mismatch"));
    }
    // A flood of replies cannot drain the model budget: at most a few readings per order
    // (and so per supplier) a day.
    let recent: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM agent_runs
         WHERE agent = 'chasing' AND input ? 'document_id' AND input ->> 'order_id' = $1
           AND id <> $2 AND started_at > now() - interval '1 day'
           AND error_code IS DISTINCT FROM 'reply_rate_limited'",
    )
    .bind(order_id.to_string())
    .bind(run.id)
    .fetch_one(&mut *tx)
    .await?;
    if !rules::reply_within_daily_limit(recent) {
        return Err(AgentError::Fatal("reply_rate_limited"));
    }
    let ordered: i64 = sqlx::query_scalar(
        "SELECT coalesce(sum(quantity), 0)::bigint FROM order_items WHERE order_id = $1",
    )
    .bind(order_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    let hash: [u8; 32] = sha
        .try_into()
        .map_err(|_| AgentError::Fatal("document_hash"))?;
    let bytes = cx
        .state
        .storage
        .get(&hash)
        .await
        .map_err(|_| AgentError::Retry("storage"))?;
    let text = crate::intake::quote_reply_text("reply.eml", &bytes)
        .await
        .map_err(|_| AgentError::Fatal("reply_unreadable"))?;
    let text: String = text
        .chars()
        .take(procure_ai::extract::MAX_INPUT_CHARS)
        .collect();
    let rcx = ReplyContext {
        po_number: order.po_number.clone(),
        ordered_quantity: u32::try_from(ordered).unwrap_or(u32::MAX),
        today: rules::jst_date(Utc::now()),
    };
    let outcome = {
        let _permit = cx
            .state
            .ai_permits
            .acquire()
            .await
            .map_err(|_| AgentError::Retry("ai_permits"))?;
        reply::parse_supplier_reply(cx.ai.as_ref(), &text, &rcx).await?
    };
    let r = outcome.value;
    let p = outcome.provenance;
    let suggestion = Uuid::now_v7();
    let mut tx = cx.tx().await?;
    sqlx::query(
        "INSERT INTO ai_suggestions (id, org_id, capability, subject_type, subject_id, supplier_id,
                                     status, input_text, input_sha256, output, provider, model,
                                     prompt_version, prompt_sha256, latency_ms, requested_by,
                                     input_source)
         VALUES ($1, $2, $3, 'purchase_order', $4, $5, 'pending', $6, $7, $8, $9, $10, $11, $12,
                 $13, NULL, 'file')",
    )
    .bind(suggestion)
    .bind(cx.org_id)
    .bind(reply::PARSE_SUPPLIER_REPLY)
    .bind(order.id)
    .bind(supplier_id)
    .bind(&text)
    .bind(&p.input_sha256)
    .bind(serde_json::to_value(&r).map_err(|_| AgentError::Fatal("suggestion"))?)
    .bind(&p.provider)
    .bind(&p.model)
    .bind(p.prompt_version)
    .bind(&p.prompt_sha256)
    .bind(i64::try_from(p.latency_ms).unwrap_or(i64::MAX))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Outcome {
        output: json!({ "document_id": document_id, "order_id": order.id,
                        "suggestion_id": suggestion, "flags": r.flags }),
        exceptions: vec![reply_exception(&order, document_id, &r, suggestion)],
        model_calls: 1,
        tx: None,
    })
}

// ---------------------------------------------------------------------------------------
// Actions

/// Supplier follow-ups and delivery dates are our staff's work (as for the order email and
/// the delivery-date route): an assigned operator with `OperateRequests`.
fn require_staff(actor: &Actor<'_>) -> ApiResult<()> {
    actor.require(Permission::OperateRequests)?;
    if common::side(actor.access)? != Side::Operator {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

/// The order the action is about: the exception's own subject, still open.
async fn action_order(tx: &mut Tx, ex: &AgentException, action: &Value) -> ApiResult<OrderRow> {
    let id = action["order_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or(ApiError::Validation("order_id"))?;
    if Some(id) != ex.subject_id {
        return Err(ApiError::Validation("order_id"));
    }
    open_orders(tx, Some(id))
        .await?
        .pop()
        .ok_or(ApiError::Conflict("invalid_transition"))
}

fn action_date(action: &Value) -> ApiResult<Option<NaiveDate>> {
    match &action["date"] {
        Value::Null => Ok(None),
        Value::String(s) => {
            let d = NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .map_err(|_| ApiError::Validation("date"))?;
            if !(2000..=2100).contains(&chrono::Datelike::year(&d)) {
                return Err(ApiError::Validation("date"));
            }
            Ok(Some(d))
        }
        _ => Err(ApiError::Validation("date")),
    }
}

async fn send_chase(
    tx: &mut Tx,
    ex: &AgentException,
    action: &Value,
    actor: &Actor<'_>,
) -> ApiResult<()> {
    require_staff(actor)?;
    // What it is (order, type, attempt) is the proposal's; a person may edit only the text.
    let proposed = ex
        .proposed_action
        .as_ref()
        .ok_or(ApiError::Validation("action"))?;
    for k in ["order_id", "chase_type", "attempt"] {
        if action[k] != proposed[k] {
            return Err(ApiError::Validation(if k == "order_id" {
                "order_id"
            } else {
                "action"
            }));
        }
    }
    let t = action["chase_type"]
        .as_str()
        .and_then(ChaseType::from_code)
        .ok_or(ApiError::Validation("chase_type"))?;
    let attempt = action["attempt"]
        .as_i64()
        .and_then(|a| i32::try_from(a).ok())
        .filter(|a| (1..=10).contains(a))
        .ok_or(ApiError::Validation("attempt"))?;
    let subject = common::required(
        action["draft"]["subject"].as_str().unwrap_or(""),
        MAX_SUBJECT,
        false,
        "subject",
    )?;
    let body = common::required(
        action["draft"]["body"].as_str().unwrap_or(""),
        MAX_BODY,
        true,
        "body",
    )?;
    let o = action_order(tx, ex, action).await?;
    let channel = queue_chase(
        actor.state,
        tx,
        ex.org_id,
        &o,
        t,
        attempt,
        &subject,
        &body,
        Some(actor.principal.user_id),
    )
    .await?;
    Audit {
        org_id: ex.org_id,
        p: actor.principal,
    }
    .record(
        tx,
        "order.follow_up_sent",
        "purchase_order",
        o.id,
        json!({ "chase_type": t.code(), "attempt": attempt, "channel": channel,
                "exception_id": ex.id, "edited": action != proposed }),
    )
    .await
}

/// The supplier acknowledged the order: recorded once, and its open acknowledgement and
/// delivery-date follow-ups are closed as handled.
async fn acknowledge(
    tx: &mut Tx,
    ex: &AgentException,
    o: &OrderRow,
    actor: &Actor<'_>,
) -> ApiResult<()> {
    let n = sqlx::query(
        "UPDATE purchase_orders SET acknowledged_at = now(), acknowledged_by = $2
         WHERE id = $1 AND acknowledged_at IS NULL",
    )
    .bind(o.id)
    .bind(actor.principal.user_id)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if n == 1 {
        Audit {
            org_id: ex.org_id,
            p: actor.principal,
        }
        .record(
            tx,
            "order.acknowledged",
            "purchase_order",
            o.id,
            json!({ "exception_id": ex.id }),
        )
        .await?;
    }
    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT dedupe_key FROM agent_exceptions
         WHERE agent = 'chasing' AND kind = 'chase_due' AND status = 'open' AND subject_id = $1
           AND params ->> 'chase_type' IN ('order_ack', 'delivery_date')",
    )
    .bind(o.id)
    .fetch_all(&mut **tx)
    .await?;
    for k in keys {
        super::resolve_elsewhere(tx, ex.org_id, &k).await?;
    }
    if let Some(s) = ex.suggestion_id {
        sqlx::query(
            "UPDATE ai_suggestions SET status = 'accepted', decided_by = $2, decided_at = now()
             WHERE id = $1 AND status = 'pending'",
        )
        .bind(s)
        .bind(actor.principal.user_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn record_promised_date(
    tx: &mut Tx,
    ex: &AgentException,
    action: &Value,
    actor: &Actor<'_>,
) -> ApiResult<()> {
    require_staff(actor)?;
    let date = action_date(action)?;
    let o = action_order(tx, ex, action).await?;
    if let Some(d) = date {
        let req = lock_request(tx, o.request_id, View::All).await?;
        if req.status()? != RequestStatus::Ordered {
            return Err(ApiError::Conflict("invalid_transition"));
        }
        // A date already known is changed only as a confirmed date.
        let n = sqlx::query(
            "UPDATE purchase_orders SET promised_on = $2 WHERE id = $1 AND promised_on IS NULL",
        )
        .bind(o.id)
        .bind(d)
        .execute(&mut **tx)
        .await?
        .rows_affected();
        if n == 0 {
            return Err(ApiError::Conflict("promised_date_known"));
        }
        Audit {
            org_id: ex.org_id,
            p: actor.principal,
        }
        .record(
            tx,
            "order.promised_date_recorded",
            "purchase_order",
            o.id,
            json!({ "promised_on": d, "exception_id": ex.id }),
        )
        .await?;
        crate::api::orders::open_delivery_check(tx, ex.org_id, o.request_id, o.id).await?;
    }
    acknowledge(tx, ex, &o, actor).await
}

async fn confirm_delivery_date(
    tx: &mut Tx,
    ex: &AgentException,
    action: &Value,
    actor: &Actor<'_>,
) -> ApiResult<()> {
    require_staff(actor)?;
    let date = action_date(action)?.ok_or(ApiError::Validation("date"))?;
    let note = common::text(action["note"].as_str().unwrap_or(""), 2000, true, "note")?;
    let o = action_order(tx, ex, action).await?;
    crate::api::orders::record_confirmed_date(tx, ex.org_id, actor.principal, o.id, date, &note)
        .await?;
    acknowledge(tx, ex, &o, actor).await
}

impl Agent for Chasing {
    fn name(&self) -> &'static str {
        NAME
    }

    fn run<'a>(
        &'a self,
        cx: &'a AgentCx,
        run: &'a AgentRun,
    ) -> BoxFuture<'a, Result<Outcome, AgentError>> {
        Box::pin(async move {
            if run.input.get("document_id").is_some() {
                read_reply(cx, run).await
            } else {
                sweep(cx, Utc::now()).await
            }
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
            match action["type"].as_str() {
                Some("send_chase") => send_chase(tx, ex, action, actor).await,
                Some("record_promised_date") => record_promised_date(tx, ex, action, actor).await,
                Some("confirm_delivery_date") => confirm_delivery_date(tx, ex, action, actor).await,
                _ => Err(ApiError::Validation("unknown_action")),
            }
        })
    }

    fn schedule(&self) -> Option<Duration> {
        Some(SWEEP_EVERY)
    }
}

/// Settings of the organisation in `tx`'s context, for the API.
pub(crate) async fn load_settings(
    tx: &mut Tx,
    org_id: Uuid,
) -> Result<(Cadence, Option<Uuid>, Option<DateTime<Utc>>), sqlx::Error> {
    let meta: Option<(Option<Uuid>, DateTime<Utc>)> =
        sqlx::query_as("SELECT updated_by, updated_at FROM chase_settings WHERE org_id = $1")
            .bind(org_id)
            .fetch_optional(&mut **tx)
            .await?;
    let c = cadence(tx, org_id).await?;
    Ok((c, meta.and_then(|m| m.0), meta.map(|m| m.1)))
}

pub(crate) async fn save_settings(
    tx: &mut db::Tx,
    org_id: Uuid,
    c: &Cadence,
    by: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO chase_settings (org_id, ack_days, delivery_date_days, overdue_days,
                                     invoice_days, repeat_days, max_attempts, updated_by,
                                     updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now())
         ON CONFLICT (org_id) DO UPDATE SET
             ack_days = EXCLUDED.ack_days, delivery_date_days = EXCLUDED.delivery_date_days,
             overdue_days = EXCLUDED.overdue_days, invoice_days = EXCLUDED.invoice_days,
             repeat_days = EXCLUDED.repeat_days, max_attempts = EXCLUDED.max_attempts,
             updated_by = EXCLUDED.updated_by, updated_at = now()",
    )
    .bind(org_id)
    .bind(c.ack_days)
    .bind(c.delivery_date_days)
    .bind(c.overdue_days)
    .bind(c.invoice_days)
    .bind(c.repeat_days)
    .bind(c.max_attempts)
    .bind(by)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}
