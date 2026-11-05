//! `cost_analyst` (L5): when quotations come in for an RFQ, compares each supplier's current
//! quotation with the factory's own lowest price, the price trend, the item's should-cost and
//! the other quotations (`api::cost::quotation_facts`, deterministic). When a quoted price is
//! above the should-cost or the lowest price paid by more than the threshold, it raises a
//! `negotiation_opportunity` exception proposing to save a negotiation brief (or, when the
//! supplier already has one, to open a negotiation task for our staff).
//!
//! It never changes a quotation, a comparison or a recommendation, and makes no model call:
//! the narrative of a brief is drafted on request by a person (`api::cost`).

use futures_util::future::BoxFuture;
use procure_domain::agents::Severity;
use procure_domain::rbac::{Access, Permission};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{Actor, Agent, AgentCx, AgentError, AgentException, AgentRun, NewException, Outcome};
use crate::api::common::Audit;
use crate::api::cost::{self, QuotationFacts};
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiResult};

pub const NAME: &str = "cost_analyst";
pub const KIND: &str = "negotiation_opportunity";
pub const TITLE_KEY: &str = "cost.negotiationOpportunity.title";

/// One open finding per supplier and RFQ; a revised quotation refreshes it.
pub fn dedupe_key(rfq_id: Uuid, supplier_id: Uuid) -> String {
    format!("{NAME}:rfq:{rfq_id}:supplier:{supplier_id}")
}

/// The run key: quotations recorded while a run waits are covered by that run.
pub fn run_key(rfq_id: Uuid) -> String {
    format!("{NAME}:rfq:{rfq_id}")
}

pub struct CostAnalyst;

fn max_of(facts: &QuotationFacts, f: impl Fn(&cost::LineFactsView) -> Option<i64>) -> Value {
    facts
        .lines
        .iter()
        .filter(|l| !l.signals.is_empty())
        .filter_map(f)
        .max()
        .map_or(Value::Null, Value::from)
}

fn exception(facts: &QuotationFacts, brief: Option<Uuid>) -> NewException {
    let mut ex = NewException::new(
        KIND,
        Severity::Warning,
        TITLE_KEY,
        &dedupe_key(facts.rfq_id, facts.supplier_id),
    );
    ex.subject_type = Some("quotation".into());
    ex.subject_id = Some(facts.quotation_id);
    ex.request_id = Some(facts.request_id);
    ex.params = json!({
        "rfq_number": facts.rfq_number,
        "supplier_name": facts.supplier_name,
        "currency": facts.currency,
        "quoted_total_minor": facts.quoted_total_minor,
        "savings_opportunity_minor": facts.savings_opportunity_minor,
        "lines_flagged": facts.lines.iter().filter(|l| !l.signals.is_empty()).count(),
        "max_should_cost_gap_bps": max_of(facts, |l| l.should_cost_gap_bps),
        "max_history_gap_bps": max_of(facts, |l| l.history_gap_bps),
        "signals": facts.signals,
    });
    ex.proposed_action = Some(match brief {
        None => json!({ "type": "save_brief", "quotation_id": facts.quotation_id }),
        Some(b) => json!({ "type": "open_negotiation_task", "quotation_id": facts.quotation_id,
                           "brief_id": b }),
    });
    ex
}

async fn analyse(cx: &AgentCx, rfq_id: Uuid) -> ApiResult<Outcome> {
    let mut tx = cx.tx().await?;
    let quotations: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM quotations WHERE rfq_id = $1 AND superseded_at IS NULL ORDER BY id",
    )
    .bind(rfq_id)
    .fetch_all(&mut *tx)
    .await?;
    let mut exceptions = Vec::new();
    let mut summary = Vec::new();
    for q in quotations {
        let Some(facts) = cost::quotation_facts(&mut tx, q).await? else {
            continue;
        };
        summary.push(json!({
            "quotation_id": q,
            "supplier_id": facts.supplier_id,
            "signals": facts.signals,
            "savings_opportunity_minor": facts.savings_opportunity_minor,
        }));
        if facts.signals.is_empty() {
            // A revised quotation without a gap closes the earlier finding.
            super::resolve_elsewhere(
                &mut tx,
                cx.org_id,
                &dedupe_key(facts.rfq_id, facts.supplier_id),
            )
            .await?;
            continue;
        }
        let brief: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM negotiation_briefs WHERE rfq_id = $1 AND supplier_id = $2
             ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(facts.rfq_id)
        .bind(facts.supplier_id)
        .fetch_optional(&mut *tx)
        .await?;
        exceptions.push(exception(&facts, brief));
    }
    tx.commit().await?;
    Ok(Outcome {
        output: json!({ "rfq_id": rfq_id, "quotations": summary, "flagged": exceptions.len() }),
        exceptions,
        model_calls: 0,
        tx: None,
    })
}

impl Agent for CostAnalyst {
    fn name(&self) -> &'static str {
        NAME
    }

    fn run<'a>(
        &'a self,
        cx: &'a AgentCx,
        run: &'a AgentRun,
    ) -> BoxFuture<'a, Result<Outcome, AgentError>> {
        Box::pin(async move {
            let rfq_id = match (run.subject_type.as_deref(), run.subject_id) {
                (Some("rfq"), Some(id)) => id,
                _ => return Err(AgentError::Fatal("subject_not_rfq")),
            };
            analyse(cx, rfq_id).await.map_err(|e| match e {
                ApiError::Internal(_) => AgentError::Retry("facts_unavailable"),
                _ => AgentError::Fatal("facts_invalid"),
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
            // The action is about the exception's quotation, whatever an edit says.
            let quotation = ex.subject_id.ok_or(ApiError::Validation("action"))?;
            if action
                .get("quotation_id")
                .is_some_and(|q| q.as_str() != Some(quotation.to_string().as_str()))
            {
                return Err(ApiError::Validation("quotation_id"));
            }
            match action.get("type").and_then(Value::as_str) {
                Some("save_brief") => save_brief(tx, ex, quotation, actor).await,
                Some("open_negotiation_task") => open_task(tx, ex, quotation, action, actor).await,
                _ => Err(ApiError::Validation("unknown_action")),
            }
        })
    }
}

async fn save_brief(
    tx: &mut Tx,
    ex: &AgentException,
    quotation: Uuid,
    actor: &Actor<'_>,
) -> ApiResult<()> {
    actor.require(Permission::ManagePurchaseHistory)?;
    // Facts as they are now, not as the run saw them.
    let facts = cost::quotation_facts(tx, quotation)
        .await?
        .ok_or(ApiError::NotFound)?;
    cost::save_brief(tx, ex.org_id, actor.principal, &facts, Some(ex.id)).await?;
    Ok(())
}

/// A staff task to negotiate with the supplier. Tasks are Chuo-Sogo's internal work list:
/// only assigned staff may open one; it is audited in the platform log.
async fn open_task(
    tx: &mut Tx,
    ex: &AgentException,
    quotation: Uuid,
    action: &Value,
    actor: &Actor<'_>,
) -> ApiResult<()> {
    actor.require(Permission::OperateRequests)?;
    if !matches!(actor.access, Access::Assigned(_)) {
        return Err(ApiError::Forbidden);
    }
    let platform = actor
        .principal
        .memberships
        .iter()
        .find(|m| m.role.is_internal())
        .map(|m| m.org_id)
        .ok_or(ApiError::Forbidden)?;
    let facts = cost::quotation_facts(tx, quotation)
        .await?
        .ok_or(ApiError::NotFound)?;
    let title = match action.get("title").and_then(Value::as_str) {
        Some(t) => crate::api::common::required(t, 200, false, "title")?,
        None => format!(
            "Negotiate price: {} / {}",
            facts.rfq_number, facts.supplier_name
        )
        .chars()
        .take(200)
        .collect(),
    };
    // The staff scope is on only for this insert (the task is a staff-only row); the
    // decision's transaction continues with the scope it had.
    let prev: String =
        sqlx::query_scalar("SELECT coalesce(current_setting('app.staff', true), '')")
            .fetch_one(&mut **tx)
            .await?;
    db::enable_staff_scope(tx).await?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tasks (id, org_id, request_id, rfq_id, kind, title, priority, created_by)
         VALUES ($1, $2, $3, $4, 'negotiate', $5, 'normal', $6)",
    )
    .bind(id)
    .bind(ex.org_id)
    .bind(facts.request_id)
    .bind(facts.rfq_id)
    .bind(&title)
    .bind(actor.principal.user_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query("SELECT set_config('app.staff', $1, true)")
        .bind(&prev)
        .execute(&mut **tx)
        .await?;
    Audit {
        org_id: platform,
        p: actor.principal,
    }
    .record(
        tx,
        "task.created",
        "task",
        id,
        json!({ "kind": "negotiate", "request": facts.request_id, "rfq": facts.rfq_id,
                "quotation": quotation, "exception": ex.id,
                "customer_org": ex.org_id.to_string() }),
    )
    .await?;
    db::set_org(tx, ex.org_id).await?;
    Ok(())
}
