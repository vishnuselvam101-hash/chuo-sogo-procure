//! Customer approval policies, operator recommendations and the customer's human
//! approval decisions. Chuo-Sogo staff recommend; only customer users with
//! ApproveRecommendation decide, under the policy snapshot taken when it was sent.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use procure_domain::approval::{self, Decision, Outcome, Policy, Tier};
use procure_domain::money::{Currency, Money};
use procure_domain::rbac::{self, Access, Permission};
use procure_domain::workflow::{RequestStatus, Side};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::types::Json as DbJson;
use uuid::Uuid;

use crate::AppState;
use crate::api::common::{self, Audit, View, lock_request, side, transition};
use crate::api::rfqs::ComparisonResult;
use crate::api::suppliers::IdResponse;
use crate::db;
use crate::error::{ApiError, ApiJson, ApiPath, ApiResult, ErrorBody};
use crate::principal::Principal;

#[derive(Serialize, Deserialize, Clone, Copy, utoipa::ToSchema)]
pub struct TierView {
    /// Threshold in minor units of the policy currency (inclusive).
    pub min_minor: i64,
    pub approvals: u8,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct PolicyView {
    pub currency: String,
    pub tiers: Vec<TierView>,
    pub allow_self_approval: bool,
    /// 0 for the built-in default (one approval, never by the requester).
    pub version: i32,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct PolicyRow {
    tiers: DbJson<Vec<TierView>>,
    allow_self_approval: bool,
    version: i32,
}

fn to_policy(currency: Currency, tiers: &[TierView], allow_self: bool) -> ApiResult<Policy> {
    let tiers = tiers
        .iter()
        .map(|t| Tier {
            min: Money::from_minor(t.min_minor, currency),
            approvals: t.approvals,
        })
        .collect();
    Policy::new(currency, tiers, allow_self).map_err(|e| ApiError::Validation(e.code()))
}

/// The organisation's policy for `currency`, or the built-in default.
async fn load_policy(
    tx: &mut db::Tx,
    org_id: Uuid,
    currency: Currency,
) -> ApiResult<(Policy, i32)> {
    let row = sqlx::query_as::<_, PolicyRow>(
        "SELECT tiers, allow_self_approval, version FROM approval_policies
         WHERE org_id = $1 AND currency = $2",
    )
    .bind(org_id)
    .bind(currency.code())
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        Some(r) => Ok((
            to_policy(currency, &r.tiers.0, r.allow_self_approval)?,
            r.version,
        )),
        None => Ok((Policy::default_for(currency), 0)),
    }
}

async fn require_any(
    p: &Principal,
    state: &AppState,
    org_id: Uuid,
    perms: &[Permission],
) -> ApiResult<Access> {
    let access = p.access(state, org_id).await?;
    if access == Access::None {
        return Err(ApiError::NotFound);
    }
    if perms.iter().any(|perm| rbac::allows(access, *perm)) {
        Ok(access)
    } else {
        Err(ApiError::Forbidden)
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/approval-policies",
    tag = "approvals",
    params(("org_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Configured policies per currency", body = [PolicyView]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list_policies(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
) -> ApiResult<Json<Vec<PolicyView>>> {
    require_any(
        &p,
        &state,
        org_id,
        &[
            Permission::ManageApprovalPolicy,
            Permission::ApproveRecommendation,
            Permission::OperateRequests,
        ],
    )
    .await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let rows = sqlx::query_as::<_, PolicyListRow>(
        "SELECT currency, tiers, allow_self_approval, version, updated_at FROM approval_policies
         WHERE org_id = $1 ORDER BY currency",
    )
    .bind(org_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| PolicyView {
                currency: r.currency,
                tiers: r.tiers.0,
                allow_self_approval: r.allow_self_approval,
                version: r.version,
                updated_at: Some(r.updated_at),
            })
            .collect(),
    ))
}

#[derive(sqlx::FromRow)]
struct PolicyListRow {
    currency: String,
    tiers: DbJson<Vec<TierView>>,
    allow_self_approval: bool,
    version: i32,
    updated_at: DateTime<Utc>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct PolicyInput {
    pub tiers: Vec<TierView>,
    #[serde(default)]
    pub allow_self_approval: bool,
}

#[utoipa::path(
    put,
    path = "/api/v1/orgs/{org_id}/approval-policies/{currency}",
    tag = "approvals",
    params(("org_id" = Uuid, Path), ("currency" = String, Path)),
    request_body = PolicyInput,
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Policy saved as a new version", body = PolicyView),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 422, description = "Invalid policy", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn put_policy(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, currency)): ApiPath<(Uuid, String)>,
    ApiJson(body): ApiJson<PolicyInput>,
) -> ApiResult<Json<PolicyView>> {
    p.require(&state, org_id, Permission::ManageApprovalPolicy)
        .await?;
    let currency = common::currency(&currency)?;
    let policy = to_policy(currency, &body.tiers, body.allow_self_approval)?;
    let tiers: Vec<TierView> = policy
        .tiers()
        .iter()
        .map(|t| TierView {
            min_minor: t.min.minor(),
            approvals: t.approvals,
        })
        .collect();
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let (previous, _) = load_policy(&mut tx, org_id, currency).await?;
    let (version, updated_at): (i32, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO approval_policies AS a (org_id, currency, tiers, allow_self_approval, updated_by)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (org_id, currency) DO UPDATE
             SET tiers = EXCLUDED.tiers, allow_self_approval = EXCLUDED.allow_self_approval,
                 updated_by = EXCLUDED.updated_by, updated_at = now(), version = a.version + 1
         RETURNING version, updated_at",
    )
    .bind(org_id)
    .bind(currency.code())
    .bind(DbJson(&tiers))
    .bind(policy.allow_self_approval())
    .bind(p.user_id)
    .fetch_one(&mut *tx)
    .await?;
    let old_tiers: Vec<TierView> = previous
        .tiers()
        .iter()
        .map(|t| TierView {
            min_minor: t.min.minor(),
            approvals: t.approvals,
        })
        .collect();
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "approval_policy.updated",
            "approval_policy",
            org_id,
            json!({ "currency": currency.code(), "version": version,
                    "from": { "tiers": old_tiers, "allow_self_approval": previous.allow_self_approval() },
                    "to": { "tiers": tiers, "allow_self_approval": policy.allow_self_approval() } }),
        )
        .await?;
    tx.commit().await?;
    Ok(Json(PolicyView {
        currency: currency.code().to_string(),
        tiers,
        allow_self_approval: policy.allow_self_approval(),
        version,
        updated_at: Some(updated_at),
    }))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct RecommendBody {
    pub comparison_id: Uuid,
    pub quotation_id: Uuid,
    /// Required when recommending anything other than the top-ranked quotation.
    #[serde(default)]
    pub reason: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/recommendations",
    tag = "approvals",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    request_body = RecommendBody,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Recommendation created (not yet sent to the customer)", body = IdResponse),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown request or not assigned", body = ErrorBody),
        (status = 409, description = "Stale comparison, unapproved supplier or wrong state", body = ErrorBody),
        (status = 422, description = "Quotation not compliant, or reason missing", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn recommend(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<RecommendBody>,
) -> ApiResult<(StatusCode, Json<IdResponse>)> {
    let access = p
        .require(&state, org_id, Permission::OperateRequests)
        .await?;
    if side(access)? != Side::Operator {
        return Err(ApiError::Forbidden);
    }
    let reason = common::text(&body.reason, 2000, true, "reason")?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let req = lock_request(&mut tx, request_id, View::All).await?;
    let results: DbJson<Vec<ComparisonResult>> = sqlx::query_scalar(
        "SELECT c.results FROM comparison_runs c JOIN rfqs f ON f.id = c.rfq_id
         WHERE c.id = $1 AND f.request_id = $2",
    )
    .bind(body.comparison_id)
    .bind(req.id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    if comparison_is_stale(&mut tx, body.comparison_id).await? {
        return Err(ApiError::Conflict("stale_comparison"));
    }
    let chosen = results
        .0
        .iter()
        .find(|r| r.quotation_id == body.quotation_id)
        .ok_or(ApiError::Validation("quotation_not_in_comparison"))?;
    let (Some(total), Some(rank), "compliant") =
        (chosen.total_minor, chosen.rank, chosen.compliance.as_str())
    else {
        return Err(ApiError::Validation("quotation_not_compliant"));
    };
    if rank != 1 && reason.is_empty() {
        return Err(ApiError::Validation("reason_required"));
    }
    let approved: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM customer_supplier_approvals
                        WHERE org_id = $1 AND supplier_id = $2 AND status = 'approved')",
    )
    .bind(org_id)
    .bind(chosen.supplier_id)
    .fetch_one(&mut *tx)
    .await?;
    if !approved {
        return Err(ApiError::Conflict("supplier_not_approved"));
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO recommendations (id, org_id, request_id, comparison_run_id, quotation_id,
                                      currency, total_minor, rank, reason, recommended_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(id)
    .bind(org_id)
    .bind(req.id)
    .bind(body.comparison_id)
    .bind(body.quotation_id)
    .bind(req.currency()?.code())
    .bind(total)
    .bind(i32::try_from(rank).map_err(|_| ApiError::Validation("rank"))?)
    .bind(&reason)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    transition(
        &mut tx,
        &Audit { org_id, p: &p },
        &req,
        RequestStatus::Recommended,
        Side::Operator,
        "recommendation.created",
        json!({ "recommendation": id, "quotation": body.quotation_id, "supplier": chosen.supplier_id,
                "total_minor": total, "rank": rank, "reason": reason }),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(IdResponse { id })))
}

/// A comparison no longer describes reality if the RFQ's current quotations differ from
/// the ones it compared (revision, new quotation) or it was run on an earlier day (validity
/// dates may have passed). The request row is locked by the caller, so quotations cannot
/// change concurrently (recording locks the RFQ, then the request).
async fn comparison_is_stale(tx: &mut db::Tx, comparison_id: Uuid) -> ApiResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT c.as_of <> (now() AT TIME ZONE o.timezone)::date
                OR c.quotation_ids::uuid[] <> coalesce(
                    (SELECT array_agg(q.id ORDER BY q.id) FROM quotations q
                     WHERE q.rfq_id = c.rfq_id AND q.superseded_at IS NULL), '{}')
         FROM comparison_runs c JOIN organisations o ON o.id = c.org_id WHERE c.id = $1",
    )
    .bind(comparison_id)
    .fetch_one(&mut **tx)
    .await?)
}

/// Re-checked before sending and before every decision: the recommended quotation is still
/// current and its supplier is still verified and approved by this customer.
async fn ensure_still_valid(tx: &mut db::Tx, org_id: Uuid, rec: &RecRow) -> ApiResult<()> {
    let (superseded, supplier_ok): (bool, bool) = sqlx::query_as(
        "SELECT q.superseded_at IS NOT NULL,
                EXISTS (SELECT 1 FROM customer_supplier_approvals a JOIN suppliers s ON s.id = a.supplier_id
                        WHERE a.org_id = $2 AND a.supplier_id = x.supplier_id
                          AND a.status = 'approved' AND s.status = 'verified')
         FROM quotations q JOIN rfq_recipients x ON x.id = q.rfq_recipient_id WHERE q.id = $1",
    )
    .bind(rec.quotation_id)
    .bind(org_id)
    .fetch_one(&mut **tx)
    .await?;
    if superseded {
        return Err(ApiError::Conflict("stale_recommendation"));
    }
    if !supplier_ok {
        return Err(ApiError::Conflict("supplier_not_approved"));
    }
    Ok(())
}

#[derive(sqlx::FromRow)]
struct RecRow {
    id: Uuid,
    quotation_id: Uuid,
    status: String,
    currency: String,
    total_minor: i64,
    required_approvals: Option<i32>,
    allow_self_approval: Option<bool>,
}

async fn lock_recommendation(tx: &mut db::Tx, id: Uuid) -> ApiResult<RecRow> {
    sqlx::query_as::<_, RecRow>(
        "SELECT id, quotation_id, status, currency, total_minor, required_approvals, allow_self_approval
         FROM recommendations WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)
}

async fn recommendation_request(tx: &mut db::Tx, id: Uuid) -> ApiResult<Uuid> {
    sqlx::query_scalar("SELECT request_id FROM recommendations WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/recommendations/{recommendation_id}/send",
    tag = "approvals",
    params(("org_id" = Uuid, Path), ("recommendation_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Sent to the customer for approval under the current policy"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown recommendation or not assigned", body = ErrorBody),
        (status = 409, description = "Already sent", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn send_recommendation(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, recommendation_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    let access = p
        .require(&state, org_id, Permission::OperateRequests)
        .await?;
    if side(access)? != Side::Operator {
        return Err(ApiError::Forbidden);
    }
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let request_id = recommendation_request(&mut tx, recommendation_id).await?;
    let req = lock_request(&mut tx, request_id, View::All).await?;
    let rec = lock_recommendation(&mut tx, recommendation_id).await?;
    if rec.status != "recommended" {
        return Err(ApiError::Conflict("invalid_transition"));
    }
    ensure_still_valid(&mut tx, org_id, &rec).await?;
    let currency = common::currency(&rec.currency)?;
    let (policy, version) = load_policy(&mut tx, org_id, currency).await?;
    let required = policy
        .required_approvals(Money::from_minor(rec.total_minor, currency))
        .map_err(|e| ApiError::Validation(e.code()))?;
    sqlx::query(
        "UPDATE recommendations SET status = 'awaiting_approval', required_approvals = $2,
                allow_self_approval = $3, policy_version = $4, sent_at = now()
         WHERE id = $1",
    )
    .bind(rec.id)
    .bind(i32::from(required))
    .bind(policy.allow_self_approval())
    .bind(version)
    .execute(&mut *tx)
    .await?;
    transition(
        &mut tx,
        &Audit { org_id, p: &p },
        &req,
        RequestStatus::AwaitingApproval,
        Side::Operator,
        "recommendation.sent",
        json!({ "recommendation": rec.id, "required_approvals": required,
                "policy_version": version, "allow_self_approval": policy.allow_self_approval() }),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct DecisionView {
    pub decided_by: Uuid,
    pub decided_by_name: Option<String>,
    /// `approve`, `reject` or `clarify`.
    pub decision: String,
    pub comment: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct RecommendationHeader {
    pub id: Uuid,
    pub request_id: Uuid,
    pub request_number: String,
    pub requested_by: Uuid,
    pub status: String,
    pub currency: String,
    pub total_minor: i64,
    pub rank: i32,
    pub reason: String,
    pub quotation_id: Uuid,
    pub supplier_id: Uuid,
    pub supplier_name: Option<String>,
    pub comparison_id: Uuid,
    pub required_approvals: Option<i32>,
    pub policy_version: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub sent_at: Option<DateTime<Utc>>,
    pub decided_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct RecommendationDetail {
    pub recommendation: RecommendationHeader,
    pub comparison: Vec<ComparisonResult>,
    pub decisions: Vec<DecisionView>,
    /// Approvals still needed while awaiting approval.
    pub remaining_approvals: Option<u8>,
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/recommendations/{recommendation_id}",
    tag = "approvals",
    params(("org_id" = Uuid, Path), ("recommendation_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Recommendation with its comparison and decisions", body = RecommendationDetail),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown recommendation", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn get_recommendation(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, recommendation_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<RecommendationDetail>> {
    let access = require_any(
        &p,
        &state,
        org_id,
        &[
            Permission::ReviewQuotations,
            Permission::ApproveRecommendation,
            Permission::OperateRequests,
        ],
    )
    .await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let recommendation = sqlx::query_as::<_, RecommendationHeader>(
        "SELECT rc.id, rc.request_id, r.number AS request_number, r.requested_by, rc.status,
                rc.currency, rc.total_minor, rc.rank, rc.reason, rc.quotation_id, x.supplier_id,
                s.name AS supplier_name, rc.comparison_run_id AS comparison_id,
                rc.required_approvals, rc.policy_version, rc.created_at, rc.sent_at, rc.decided_at
         FROM recommendations rc
         JOIN procurement_requests r ON r.id = rc.request_id
         JOIN quotations q ON q.id = rc.quotation_id
         JOIN rfq_recipients x ON x.id = q.rfq_recipient_id
         LEFT JOIN suppliers s ON s.id = x.supplier_id
         WHERE rc.id = $1",
    )
    .bind(recommendation_id)
    .fetch_optional(&mut *tx)
    .await?
    .filter(|r| matches!(access, Access::Assigned(_)) || r.sent_at.is_some())
    // Unsent drafts are internal to Chuo-Sogo.
    .ok_or(ApiError::NotFound)?;
    let comparison: DbJson<Vec<ComparisonResult>> =
        sqlx::query_scalar("SELECT results FROM comparison_runs WHERE id = $1")
            .bind(recommendation.comparison_id)
            .fetch_one(&mut *tx)
            .await?;
    let decisions = sqlx::query_as::<_, DecisionView>(
        "SELECT a.decided_by, u.display_name AS decided_by_name, a.decision, a.comment, a.created_at
         FROM approval_records a LEFT JOIN users u ON u.id = a.decided_by
         WHERE a.recommendation_id = $1 ORDER BY a.created_at, a.id",
    )
    .bind(recommendation_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let remaining = match (
        recommendation.status.as_str(),
        recommendation.required_approvals,
    ) {
        ("awaiting_approval", Some(req)) => {
            let ds: Vec<Decision> = decisions
                .iter()
                .filter(|d| d.decision == "approve")
                .map(|_| Decision::Approve)
                .collect();
            match approval::outcome(u8::try_from(req).unwrap_or(1), &ds) {
                Outcome::Pending { remaining } => Some(remaining),
                _ => Some(0),
            }
        }
        _ => None,
    };
    Ok(Json(RecommendationDetail {
        recommendation,
        comparison: comparison.0,
        decisions,
        remaining_approvals: remaining,
    }))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct DecisionBody {
    /// `approve`, `reject` or `clarify`.
    pub decision: String,
    /// Required for `reject` and `clarify`.
    #[serde(default)]
    pub comment: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct DecisionResponse {
    /// Recommendation status after this decision.
    pub status: String,
    pub remaining_approvals: u8,
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/recommendations/{recommendation_id}/decisions",
    tag = "approvals",
    params(("org_id" = Uuid, Path), ("recommendation_id" = Uuid, Path)),
    request_body = DecisionBody,
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Decision recorded", body = DecisionResponse),
        (status = 403, description = "Not an approver, or self-approval not allowed", body = ErrorBody),
        (status = 404, description = "Unknown recommendation", body = ErrorBody),
        (status = 409, description = "Not awaiting approval, or already decided by this user", body = ErrorBody),
        (status = 422, description = "Invalid decision or missing comment", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn decide(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, recommendation_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<DecisionBody>,
) -> ApiResult<Json<DecisionResponse>> {
    // Customer members only: rbac never grants this through a service assignment.
    let access = p
        .require(&state, org_id, Permission::ApproveRecommendation)
        .await?;
    let side = side(access)?;
    let comment = common::text(&body.comment, 2000, true, "comment")?;
    let decision = match body.decision.as_str() {
        "approve" => Some(Decision::Approve),
        "reject" => Some(Decision::Reject),
        "clarify" => None,
        _ => return Err(ApiError::Validation("decision")),
    };
    if decision != Some(Decision::Approve) && comment.is_empty() {
        return Err(ApiError::Validation("comment"));
    }
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let request_id = recommendation_request(&mut tx, recommendation_id).await?;
    let req = lock_request(&mut tx, request_id, View::All).await?;
    let rec = lock_recommendation(&mut tx, recommendation_id).await?;
    if rec.status != "awaiting_approval" {
        return Err(ApiError::Conflict("not_awaiting_approval"));
    }
    // Approving needs everything still valid; rejecting or asking a question never does.
    if decision == Some(Decision::Approve) {
        ensure_still_valid(&mut tx, org_id, &rec).await?;
    }
    let previous: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT decided_by, decision FROM approval_records WHERE recommendation_id = $1",
    )
    .bind(rec.id)
    .fetch_all(&mut *tx)
    .await?;
    let deciders: Vec<Uuid> = previous.iter().map(|(u, _)| *u).collect();
    approval::check_decider(
        rec.allow_self_approval.unwrap_or(false),
        &p.user_id,
        &req.requested_by,
        decision.unwrap_or(Decision::Reject),
        &deciders,
    )
    .map_err(|e| match e {
        approval::DecisionError::SelfApproval => ApiError::Forbidden,
        approval::DecisionError::AlreadyDecided => ApiError::Conflict(e.code()),
    })?;
    sqlx::query(
        "INSERT INTO approval_records (id, org_id, recommendation_id, decided_by, decision, comment)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(org_id)
    .bind(rec.id)
    .bind(p.user_id)
    .bind(&body.decision)
    .bind(&comment)
    .execute(&mut *tx)
    .await?;
    let audit = Audit { org_id, p: &p };
    audit
        .record(
            &mut tx,
            "approval.recorded",
            "recommendation",
            rec.id,
            json!({ "decision": body.decision, "comment": comment }),
        )
        .await?;

    let required = u8::try_from(rec.required_approvals.unwrap_or(1)).unwrap_or(1);
    let (status, remaining) = match decision {
        None => {
            sqlx::query(
                "UPDATE procurement_requests SET clarification_question = $2,
                        clarification_answer = NULL WHERE id = $1",
            )
            .bind(req.id)
            .bind(&comment)
            .execute(&mut *tx)
            .await?;
            transition(
                &mut tx,
                &audit,
                &req,
                RequestStatus::NeedsClarification,
                side,
                "request.clarification_requested",
                json!({ "question": comment, "recommendation": rec.id }),
            )
            .await?;
            ("clarification_requested", required)
        }
        Some(_) => {
            let mut all: Vec<Decision> = previous
                .iter()
                .filter_map(|(_, d)| match d.as_str() {
                    "approve" => Some(Decision::Approve),
                    "reject" => Some(Decision::Reject),
                    _ => None,
                })
                .collect();
            all.extend(decision);
            match approval::outcome(required, &all) {
                Outcome::Pending { remaining } => ("awaiting_approval", remaining),
                Outcome::Approved => {
                    transition(
                        &mut tx,
                        &audit,
                        &req,
                        RequestStatus::Approved,
                        side,
                        "request.approved",
                        json!({ "recommendation": rec.id, "approvals": required }),
                    )
                    .await?;
                    ("approved", 0)
                }
                Outcome::Rejected => {
                    transition(
                        &mut tx,
                        &audit,
                        &req,
                        RequestStatus::Rejected,
                        side,
                        "request.rejected",
                        json!({ "recommendation": rec.id, "comment": comment }),
                    )
                    .await?;
                    ("rejected", 0)
                }
            }
        }
    };
    if status != "awaiting_approval" {
        sqlx::query("UPDATE recommendations SET status = $2, decided_at = now() WHERE id = $1")
            .bind(rec.id)
            .bind(status)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(DecisionResponse {
        status: status.to_string(),
        remaining_approvals: remaining,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/recommendations/{recommendation_id}/withdraw",
    tag = "approvals",
    params(("org_id" = Uuid, Path), ("recommendation_id" = Uuid, Path)),
    request_body = crate::api::requests::ReasonBody,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Unsent recommendation withdrawn; the request returns to comparison"),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown recommendation or not assigned", body = ErrorBody),
        (status = 409, description = "Already sent to the customer", body = ErrorBody),
        (status = 422, description = "Missing reason", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn withdraw_recommendation(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, recommendation_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<crate::api::requests::ReasonBody>,
) -> ApiResult<StatusCode> {
    let access = p
        .require(&state, org_id, Permission::OperateRequests)
        .await?;
    if side(access)? != Side::Operator {
        return Err(ApiError::Forbidden);
    }
    let reason = common::required(&body.reason, 2000, true, "reason")?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let request_id = recommendation_request(&mut tx, recommendation_id).await?;
    let req = lock_request(&mut tx, request_id, View::All).await?;
    let rec = lock_recommendation(&mut tx, recommendation_id).await?;
    if rec.status != "recommended" {
        return Err(ApiError::Conflict("invalid_transition"));
    }
    sqlx::query(
        "UPDATE recommendations SET status = 'withdrawn', decided_at = now() WHERE id = $1",
    )
    .bind(rec.id)
    .execute(&mut *tx)
    .await?;
    transition(
        &mut tx,
        &Audit { org_id, p: &p },
        &req,
        RequestStatus::UnderComparison,
        Side::Operator,
        "recommendation.withdrawn",
        json!({ "recommendation": rec.id, "reason": reason }),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
