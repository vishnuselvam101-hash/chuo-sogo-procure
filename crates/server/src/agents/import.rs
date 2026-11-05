//! The `import` agent (L7, docs/agents/INTEGRATIONS.md). Runs when a file is uploaded: reads
//! the staged batch, cross-checks it against the organisation's requests and orders (unknown
//! request or PO numbers, records already entered, orders not in the right state, lines that
//! are not on the order, amounts that differ) and proposes applying it. It writes nothing:
//! applying is a person's decision, executed through `api::integrations::apply_batch` with
//! that person's permissions, all or nothing.

use futures_util::future::BoxFuture;
use procure_domain::agents::Severity;
use procure_domain::connector::{self as conn, ImportKind, KnownLine};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{Actor, Agent, AgentCx, AgentError, AgentException, AgentRun, NewException, Outcome};
use crate::api::integrations::{apply_batch, known_lines, order_by_po, record_line, staged_of};
use crate::db::Tx;
use crate::error::{ApiError, ApiResult};

pub const NAME: &str = "import";
/// Findings listed on one exception (the rest are counted).
const MAX_LISTED: usize = 50;

pub struct Import;

/// Something about one staged record. `blocking` findings would make the apply fail.
#[derive(Debug, Serialize)]
struct Finding {
    seq: i32,
    rows: Vec<i32>,
    po_number: String,
    /// `unknown_request` | `unknown_po` | `duplicate` | `not_ready` | `unknown_item` |
    /// `over_received` | `amount_mismatch`
    code: &'static str,
    blocking: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actual: Option<i64>,
}

async fn request_status(tx: &mut Tx, request_id: Uuid) -> ApiResult<String> {
    Ok(
        sqlx::query_scalar("SELECT status FROM procurement_requests WHERE id = $1")
            .bind(request_id)
            .fetch_one(&mut **tx)
            .await?,
    )
}

async fn check(tx: &mut Tx, batch_id: Uuid, kind: ImportKind) -> ApiResult<Vec<Finding>> {
    let mut out = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (seq, s) in staged_of(tx, batch_id, i64::MAX).await? {
        let mut find = |code: &'static str, blocking: bool, e: Option<i64>, a: Option<i64>| {
            out.push(Finding {
                seq,
                rows: s.rows.clone(),
                po_number: s.po_number.clone(),
                code,
                blocking,
                expected: e,
                actual: a,
            })
        };
        let key = match kind {
            ImportKind::Invoice => s.invoice_number.clone(),
            _ => s.po_number.clone(),
        };
        // The same PO (or invoice) twice in one file: the second would be refused.
        if kind != ImportKind::Receipt && seen.contains(&key) {
            find("duplicate", true, None, None);
            continue;
        }
        seen.push(key);
        match kind {
            ImportKind::Po => {
                if order_by_po(tx, &s.po_number).await?.is_some() {
                    find("duplicate", true, None, None);
                    continue;
                }
                let request: Option<(Uuid, String)> =
                    sqlx::query_as("SELECT id, status FROM procurement_requests WHERE number = $1")
                        .bind(&s.request_number)
                        .fetch_optional(&mut **tx)
                        .await?;
                let Some((request_id, status)) = request else {
                    find("unknown_request", true, None, None);
                    continue;
                };
                if status != "approved" {
                    find("not_ready", true, None, None);
                    continue;
                }
                // What the factory's PO says against what was approved (lines, without
                // shipping): a difference is for a person to look at; the order is recorded
                // from the approved quotation either way.
                let approved: Option<i64> = sqlx::query_scalar(
                    "SELECT sum(l.unit_price_minor * l.quantity)::bigint
                     FROM recommendations r JOIN quotation_lines l ON l.quotation_id = r.quotation_id
                     WHERE r.request_id = $1 AND r.status = 'approved'
                       AND r.decided_at = (SELECT max(decided_at) FROM recommendations
                                           WHERE request_id = $1 AND status = 'approved')",
                )
                .bind(request_id)
                .fetch_one(&mut **tx)
                .await?;
                let priced: Option<i64> = s.lines.iter().try_fold(0_i64, |acc, l| {
                    acc.checked_add(l.unit_price_minor?.checked_mul(i64::from(l.quantity?))?)
                });
                if let (Some(a), Some(p)) = (approved, priced)
                    && !s.lines.is_empty()
                    && a != p
                {
                    find("amount_mismatch", false, Some(a), Some(p));
                }
            }
            ImportKind::Receipt | ImportKind::Invoice => {
                let Some(order_id) = order_by_po(tx, &s.po_number).await? else {
                    find("unknown_po", true, None, None);
                    continue;
                };
                let request_id: Uuid =
                    sqlx::query_scalar("SELECT request_id FROM purchase_orders WHERE id = $1")
                        .bind(order_id)
                        .fetch_one(&mut **tx)
                        .await?;
                let status = request_status(tx, request_id).await?;
                if kind == ImportKind::Invoice {
                    let recorded: bool = sqlx::query_scalar(
                        "SELECT EXISTS (SELECT 1 FROM invoices i
                                        JOIN purchase_orders o ON o.id = $1
                                        WHERE i.supplier_id = o.supplier_id
                                          AND i.supplier_invoice_number = $2)",
                    )
                    .bind(order_id)
                    .bind(&s.invoice_number)
                    .fetch_one(&mut **tx)
                    .await?;
                    if recorded {
                        find("duplicate", true, None, None);
                        continue;
                    }
                }
                let ready = match kind {
                    ImportKind::Receipt => "ordered",
                    _ => "delivered",
                };
                if status != ready {
                    find("not_ready", true, None, None);
                    continue;
                }
                let known = known_lines(tx, order_id).await?;
                let k: Vec<KnownLine> = known.iter().map(|k| k.1.clone()).collect();
                for l in &s.lines {
                    let Some(i) = conn::match_line(&k, &record_line(l)) else {
                        // An unknown invoice line is recorded and becomes a discrepancy.
                        find("unknown_item", kind == ImportKind::Receipt, None, None);
                        continue;
                    };
                    let (item, _, ordered, price) = &known[i];
                    if kind == ImportKind::Receipt {
                        let received: i64 = sqlx::query_scalar(
                            "SELECT coalesce(sum(d.quantity), 0)::bigint FROM delivery_lines d
                             JOIN deliveries dv ON dv.id = d.delivery_id
                             WHERE d.order_item_id = $1 AND dv.reversed_at IS NULL",
                        )
                        .bind(item)
                        .fetch_one(&mut **tx)
                        .await?;
                        let q = i64::from(l.quantity.unwrap_or(0));
                        if received + q > i64::from(*ordered) {
                            find(
                                "over_received",
                                true,
                                Some(i64::from(*ordered) - received),
                                Some(q),
                            );
                        }
                    } else if l.unit_price_minor.is_some_and(|p| p != *price) {
                        find("amount_mismatch", false, Some(*price), l.unit_price_minor);
                    }
                }
            }
        }
    }
    Ok(out)
}

fn api_to_agent(e: ApiError) -> AgentError {
    match e {
        ApiError::Internal(_) => AgentError::Retry("import_check_failed"),
        _ => AgentError::Fatal("import_check_failed"),
    }
}

impl Agent for Import {
    fn name(&self) -> &'static str {
        NAME
    }

    fn run<'a>(
        &'a self,
        cx: &'a AgentCx,
        run: &'a AgentRun,
    ) -> BoxFuture<'a, Result<Outcome, AgentError>> {
        Box::pin(async move {
            let batch_id = match (run.subject_type.as_deref(), run.subject_id) {
                (Some("import_batch"), Some(id)) => id,
                _ => return Err(AgentError::Fatal("no_batch")),
            };
            let mut tx = cx.tx().await?;
            let batch: Option<(String, String, String, i32, i32)> = sqlx::query_as(
                "SELECT kind, status, file_name, records, error_rows FROM import_batches
                 WHERE id = $1 AND org_id = $2",
            )
            .bind(batch_id)
            .bind(cx.org_id)
            .fetch_optional(&mut *tx)
            .await?;
            let Some((kind, status, file_name, records, error_rows)) = batch else {
                return Err(AgentError::Fatal("unknown_batch"));
            };
            if status != "previewed" {
                return Ok(Outcome::new(
                    json!({ "batch": batch_id, "skipped": status }),
                ));
            }
            let kind = ImportKind::from_code(&kind).ok_or(AgentError::Fatal("unknown_kind"))?;
            let findings = check(&mut tx, batch_id, kind).await.map_err(api_to_agent)?;
            tx.rollback().await?;
            let blocking = findings.iter().filter(|f| f.blocking).count();
            let listed: Vec<&Finding> = findings.iter().take(MAX_LISTED).collect();
            let (ex_kind, severity, title) = if blocking > 0 {
                (
                    "import_blocked",
                    Severity::Critical,
                    "integrations.import_blocked",
                )
            } else if findings.is_empty() && error_rows == 0 {
                ("import_ready", Severity::Info, "integrations.import_ready")
            } else {
                (
                    "import_ready",
                    Severity::Warning,
                    "integrations.import_ready",
                )
            };
            let mut ex = NewException::new(
                ex_kind,
                severity,
                title,
                &format!("import:batch:{batch_id}"),
            );
            ex.subject_type = Some("import_batch".into());
            ex.subject_id = Some(batch_id);
            ex.params = json!({
                "kind": kind.code(), "file_name": file_name, "records": records,
                "error_rows": error_rows, "findings": listed, "findings_total": findings.len(),
                "blocking": blocking,
            });
            if blocking == 0 {
                ex.proposed_action = Some(json!({ "type": "apply_import", "batch_id": batch_id }));
            }
            Ok(Outcome {
                output: json!({ "batch": batch_id, "findings": findings.len(),
                                "blocking": blocking }),
                exceptions: vec![ex],
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
            if action["type"].as_str() != Some("apply_import") {
                return Err(ApiError::Validation("unknown_action"));
            }
            // Only the batch this exception is about (an edit cannot point elsewhere).
            let batch_id: Uuid = action["batch_id"]
                .as_str()
                .and_then(|s| s.parse().ok())
                .ok_or(ApiError::Validation("batch_id"))?;
            if ex.subject_type.as_deref() != Some("import_batch") || ex.subject_id != Some(batch_id)
            {
                return Err(ApiError::Validation("batch_id"));
            }
            apply_batch(tx, actor.principal, actor.access, ex.org_id, batch_id).await?;
            Ok(())
        })
    }
}
