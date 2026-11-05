//! `agent_exceptions`: the inbox of what agents found and propose. One open exception per
//! `dedupe_key` (prefix keys with the agent name, e.g. `matching:invoice:<id>`); a person
//! decides each one, and only an approve or edit makes the server act.

use chrono::{DateTime, Utc};
use procure_domain::agents::{self as rules, Decision, ExceptionStatus, Severity};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{Actor, find};
use crate::api::common::Audit;
use crate::audit::{self, Event};
use crate::db::Tx;
use crate::error::{ApiError, ApiResult};

#[derive(Debug, Clone, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct AgentException {
    pub id: Uuid,
    pub org_id: Uuid,
    pub agent: String,
    pub run_id: Option<Uuid>,
    pub kind: String,
    /// `info` | `warning` | `critical`
    pub severity: String,
    pub subject_type: Option<String>,
    pub subject_id: Option<Uuid>,
    pub request_id: Option<Uuid>,
    /// Locale key (`agents:<lane>.…`); `params` holds the values its text needs.
    pub title_key: String,
    #[schema(value_type = Object)]
    pub params: Value,
    /// `{"type": "...", ...}`; executed only after a person approves or edits it.
    #[schema(value_type = Option<Object>)]
    pub proposed_action: Option<Value>,
    pub suggestion_id: Option<Uuid>,
    /// `open` | `approved` | `edited` | `rejected` | `resolved_elsewhere`
    pub status: String,
    pub dedupe_key: String,
    pub decided_by: Option<Uuid>,
    pub decided_at: Option<DateTime<Utc>>,
    /// The action that was executed (the proposal, or the person's edit of it).
    #[schema(value_type = Option<Object>)]
    pub decided_action: Option<Value>,
    pub decision_note: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub(crate) const EXCEPTION_COLUMNS: &str = "id, org_id, agent, run_id, kind, severity, \
     subject_type, subject_id, request_id, title_key, params, proposed_action, suggestion_id, \
     status, dedupe_key, decided_by, decided_at, decided_action, decision_note, created_at, \
     updated_at";

/// A finding to put in the inbox.
#[derive(Debug, Clone)]
pub struct NewException {
    /// Short machine kind, e.g. `price_mismatch`.
    pub kind: String,
    pub severity: Severity,
    pub subject_type: Option<String>,
    pub subject_id: Option<Uuid>,
    pub request_id: Option<Uuid>,
    /// Locale key, never prose.
    pub title_key: String,
    /// Values for the title/body text (a JSON object).
    pub params: Value,
    /// `{"type": "...", ...}`, or `None` for a notice that needs no action.
    pub proposed_action: Option<Value>,
    /// The `ai_suggestions` row a model-generated proposal came from.
    pub suggestion_id: Option<Uuid>,
    /// One open exception per key and organisation.
    pub dedupe_key: String,
}

impl NewException {
    /// A notice without subject, action or suggestion; set the other fields as needed.
    pub fn new(kind: &str, severity: Severity, title_key: &str, dedupe_key: &str) -> Self {
        NewException {
            kind: kind.into(),
            severity,
            subject_type: None,
            subject_id: None,
            request_id: None,
            title_key: title_key.into(),
            params: json!({}),
            proposed_action: None,
            suggestion_id: None,
            dedupe_key: dedupe_key.into(),
        }
    }
}

/// Puts a finding in the inbox (transaction in `org_id`'s context). An open exception with
/// the same key is refreshed instead (new params/action), and nothing changes when the
/// finding is the same. Audits `agent.exception_raised` / `agent.exception_updated` as the
/// system. Returns the exception id.
pub async fn raise(
    tx: &mut Tx,
    org_id: Uuid,
    agent: &str,
    run_id: Option<Uuid>,
    ex: &NewException,
) -> Result<Uuid, sqlx::Error> {
    let changed: Option<(Uuid, bool)> = sqlx::query_as(
        "INSERT INTO agent_exceptions (id, org_id, agent, run_id, kind, severity, subject_type,
                                       subject_id, request_id, title_key, params,
                                       proposed_action, suggestion_id, dedupe_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
         ON CONFLICT (org_id, dedupe_key) WHERE status = 'open' DO UPDATE SET
             run_id = coalesce(EXCLUDED.run_id, agent_exceptions.run_id),
             severity = EXCLUDED.severity, title_key = EXCLUDED.title_key,
             params = EXCLUDED.params, proposed_action = EXCLUDED.proposed_action,
             suggestion_id = EXCLUDED.suggestion_id, updated_at = now()
         WHERE (agent_exceptions.severity, agent_exceptions.title_key, agent_exceptions.params,
                agent_exceptions.proposed_action, agent_exceptions.suggestion_id)
               IS DISTINCT FROM
               (EXCLUDED.severity, EXCLUDED.title_key, EXCLUDED.params,
                EXCLUDED.proposed_action, EXCLUDED.suggestion_id)
         RETURNING id, (xmax = 0) AS inserted",
    )
    .bind(Uuid::now_v7())
    .bind(org_id)
    .bind(agent)
    .bind(run_id)
    .bind(&ex.kind)
    .bind(ex.severity.code())
    .bind(&ex.subject_type)
    .bind(ex.subject_id)
    .bind(ex.request_id)
    .bind(&ex.title_key)
    .bind(&ex.params)
    .bind(&ex.proposed_action)
    .bind(ex.suggestion_id)
    .bind(&ex.dedupe_key)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((id, inserted)) = changed else {
        // The same finding is already open.
        return sqlx::query_scalar(
            "SELECT id FROM agent_exceptions
             WHERE org_id = $1 AND dedupe_key = $2 AND status = 'open'",
        )
        .bind(org_id)
        .bind(&ex.dedupe_key)
        .fetch_one(&mut **tx)
        .await;
    };
    audit::record(
        tx,
        Event {
            org_id,
            actor: None,
            action: if inserted {
                "agent.exception_raised"
            } else {
                "agent.exception_updated"
            },
            entity_type: "agent_exception",
            entity_id: Some(id.to_string()),
            data: json!({
                "agent": agent,
                "kind": ex.kind,
                "severity": ex.severity.code(),
                "run_id": run_id,
                "dedupe_key": ex.dedupe_key,
                "proposed_action": ex.proposed_action.as_ref().and_then(|a| a.get("type")),
            }),
            request_id: None,
        },
    )
    .await?;
    Ok(id)
}

/// Closes the open exception with this key because the situation was handled elsewhere
/// (e.g. a person fixed it by hand). Returns its id, `None` when none was open.
pub async fn resolve_elsewhere(
    tx: &mut Tx,
    org_id: Uuid,
    dedupe_key: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    let id: Option<Uuid> = sqlx::query_scalar(
        "UPDATE agent_exceptions SET status = 'resolved_elsewhere', decided_at = now(),
                updated_at = now()
         WHERE org_id = $1 AND dedupe_key = $2 AND status = 'open'
         RETURNING id",
    )
    .bind(org_id)
    .bind(dedupe_key)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(id) = id {
        audit::record(
            tx,
            Event {
                org_id,
                actor: None,
                action: "agent.exception_resolved",
                entity_type: "agent_exception",
                entity_id: Some(id.to_string()),
                data: json!({ "dedupe_key": dedupe_key }),
                request_id: None,
            },
        )
        .await?;
    }
    Ok(id)
}

/// A person's decision on an open exception (transaction in `org_id`'s context; the
/// caller has checked `DecideAgentExceptions`). Records the decision and audits
/// `agent.exception_decided`; for approve/edit, then executes the action through the
/// agent's `apply` in the same transaction, so a refused action undoes the decision too.
/// An edit may change the action's details, never its type.
pub async fn decide(
    tx: &mut Tx,
    actor: &Actor<'_>,
    org_id: Uuid,
    id: Uuid,
    decision: Decision<Value>,
    note: &str,
) -> ApiResult<AgentException> {
    let ex = sqlx::query_as::<_, AgentException>(&format!(
        "SELECT {EXCEPTION_COLUMNS} FROM agent_exceptions WHERE id = $1 AND org_id = $2
         FOR UPDATE"
    ))
    .bind(id)
    .bind(org_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    let from = ExceptionStatus::from_code(&ex.status)
        .ok_or_else(|| ApiError::internal(std::io::Error::other("unknown exception status")))?;
    rules::close(from, decision.outcome()).map_err(|_| ApiError::Conflict("exception_not_open"))?;
    let action = match &decision {
        Decision::Reject => None,
        Decision::Approve => ex.proposed_action.clone(),
        Decision::Edit(edited) => {
            let proposed_type = ex
                .proposed_action
                .as_ref()
                .and_then(|a| a.get("type"))
                .and_then(Value::as_str)
                .ok_or(ApiError::Validation("nothing_to_edit"))?;
            if !edited.is_object()
                || edited.get("type").and_then(Value::as_str) != Some(proposed_type)
            {
                return Err(ApiError::Validation("action"));
            }
            Some(edited.clone())
        }
    };
    let decided = sqlx::query_as::<_, AgentException>(&format!(
        "UPDATE agent_exceptions SET status = $2, decided_by = $3, decided_at = now(),
                decided_action = $4, decision_note = $5, updated_at = now()
         WHERE id = $1
         RETURNING {EXCEPTION_COLUMNS}"
    ))
    .bind(id)
    .bind(decision.outcome().code())
    .bind(actor.principal.user_id)
    .bind(&action)
    .bind(note)
    .fetch_one(&mut **tx)
    .await?;
    Audit {
        org_id,
        p: actor.principal,
    }
    .record(
        tx,
        "agent.exception_decided",
        "agent_exception",
        id,
        json!({
            "decision": decision.code(),
            "agent": ex.agent,
            "kind": ex.kind,
            "action": action,
        }),
    )
    .await?;
    if let (true, Some(action)) = (decision.executes(), &action) {
        let agent = find(actor.state, &ex.agent).ok_or(ApiError::Conflict("unknown_agent"))?;
        agent.apply(tx, &decided, action, actor).await?;
    }
    Ok(decided)
}
