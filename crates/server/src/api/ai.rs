//! AI suggestions: run a capability on untrusted text, store the validated result with its
//! provenance, and let a human accept or reject it. Accepting item suggestions adds items to
//! the requester's own draft; quotation suggestions only pre-fill the operator's quotation
//! form and are marked accepted when that quotation is recorded (see `rfqs::record_quotation`).

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use procure_ai::AiError;
use procure_ai::extract::{self, ItemSuggestion, Outcome, QuoteTarget};
use procure_domain::rbac::{Access, Permission};
use procure_domain::workflow::{RequestStatus, Side};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::AppState;
use crate::api::common::{self, Audit, View, lock_request, request_view, require_perm, side};
use crate::db;
use crate::error::{ApiError, ApiJson, ApiPath, ApiResult, ErrorBody};
use crate::principal::Principal;
use crate::ratelimit;

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ExtractBody {
    /// Untrusted text (customer email, supplier reply). At most 20,000 characters.
    pub text: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ExtractQuotationBody {
    pub supplier_id: Uuid,
    pub text: String,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct SuggestionView {
    pub id: Uuid,
    /// `extract_items` or `extract_quotation`.
    pub capability: String,
    pub subject_id: Uuid,
    pub supplier_id: Option<Uuid>,
    /// `pending`, `accepted`, `rejected` or `failed`.
    pub status: String,
    /// Validated output: an array of item suggestions, or a quotation suggestion. Null when failed.
    #[schema(value_type = Object)]
    pub output: Option<Value>,
    pub error_code: Option<String>,
    pub provider: String,
    pub model: String,
    pub prompt_version: String,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
}

pub(crate) const SUGGESTION_COLUMNS: &str =
    "id, capability, subject_id, supplier_id, status, output, error_code,
    provider, model, prompt_version, created_at, decided_at";

pub(crate) fn check_text(text: &str) -> ApiResult<String> {
    let t = text.trim();
    if t.is_empty() {
        return Err(ApiError::Validation("text"));
    }
    if t.chars().count() > extract::MAX_INPUT_CHARS {
        return Err(ApiError::Validation("ai_input_too_large"));
    }
    // Keep newlines/tabs (structure matters), drop other control characters.
    Ok(t.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect())
}

/// Chuo-Sogo can switch AI off per customer organisation (admin console rules).
pub(crate) async fn ai_allowed(state: &AppState, p: &Principal, org_id: Uuid) -> ApiResult<()> {
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let enabled = crate::api::admin::load_settings(&mut tx, org_id)
        .await?
        .ai_enabled;
    tx.commit().await?;
    if enabled {
        Ok(())
    } else {
        Err(ApiError::Conflict("ai_disabled"))
    }
}

pub(crate) fn rate_limit(state: &AppState, p: &Principal) -> ApiResult<()> {
    if state
        .limiter
        .check(&format!("ai:{}", p.user_id), &ratelimit::AI_PER_USER)
    {
        Ok(())
    } else {
        Err(ApiError::RateLimited)
    }
}

pub(crate) struct NewSuggestion<'a> {
    pub org_id: Uuid,
    pub capability: &'static str,
    pub subject_type: &'static str,
    pub subject_id: Uuid,
    pub supplier_id: Option<Uuid>,
    pub text: &'a str,
    /// `typed`, `voice` or `file`.
    pub source: &'static str,
}

/// Stores the outcome (success or failure) and audits it. Returns the stored view.
pub(crate) async fn store<T: Serialize>(
    state: &AppState,
    p: &Principal,
    s: NewSuggestion<'_>,
    result: &Result<Outcome<T>, AiError>,
) -> ApiResult<SuggestionView> {
    let id = Uuid::now_v7();
    let (status, output, error_code, provenance) = match result {
        Ok(o) => (
            "pending",
            Some(serde_json::to_value(&o.value).map_err(ApiError::internal)?),
            None,
            Some(&o.provenance),
        ),
        Err(e) => ("failed", None, Some(e.code()), None),
    };
    let input_sha256 = provenance
        .map(|p| p.input_sha256.clone())
        .unwrap_or_else(|| hex::encode(crate::storage::sha256(s.text.as_bytes())));
    let mut tx = db::tenant_tx(&state.pool, p.user_id, s.org_id).await?;
    let view = sqlx::query_as::<_, SuggestionView>(&format!(
        "INSERT INTO ai_suggestions (id, org_id, capability, subject_type, subject_id, supplier_id,
                                     status, input_text, input_sha256, output, error_code, provider,
                                     model, prompt_version, prompt_sha256, latency_ms, requested_by,
                                     input_source)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)
         RETURNING {SUGGESTION_COLUMNS}"
    ))
    .bind(id)
    .bind(s.org_id)
    .bind(s.capability)
    .bind(s.subject_type)
    .bind(s.subject_id)
    .bind(s.supplier_id)
    .bind(status)
    .bind(s.text)
    .bind(&input_sha256)
    .bind(&output)
    .bind(error_code)
    .bind(state.ai.name())
    .bind(state.ai.model())
    .bind(provenance.map_or(s.capability, |p| p.prompt_version))
    .bind(provenance.map(|p| p.prompt_sha256.clone()))
    .bind(provenance.and_then(|p| i64::try_from(p.latency_ms).ok()))
    .bind(p.user_id)
    .bind(s.source)
    .fetch_one(&mut *tx)
    .await?;
    Audit {
        org_id: s.org_id,
        p,
    }
    .record(
        &mut tx,
        "ai.suggested",
        "ai_suggestion",
        id,
        json!({ "capability": s.capability, "subject": s.subject_id, "status": status,
                "source": s.source,
                "error": error_code, "provider": state.ai.name(), "model": state.ai.model(),
                "prompt_version": provenance.map(|p| p.prompt_version),
                "input_sha256": input_sha256 }),
    )
    .await?;
    tx.commit().await?;
    Ok(view)
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/ai/extract-items",
    tag = "ai",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path)),
    request_body = ExtractBody,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Suggestion stored (status pending, or failed with error_code)", body = SuggestionView),
        (status = 403, description = "Not the requester", body = ErrorBody),
        (status = 404, description = "Unknown or not visible", body = ErrorBody),
        (status = 409, description = "Request not editable", body = ErrorBody),
        (status = 422, description = "Empty or too long text", body = ErrorBody),
        (status = 429, description = "AI rate limit reached", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn extract_items(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<ExtractBody>,
) -> ApiResult<(StatusCode, Json<SuggestionView>)> {
    let (access, view) = request_view(&p, &state, org_id).await?;
    require_perm(access, Permission::SubmitRequest)?;
    let text = check_text(&body.text)?;
    {
        let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
        let req = lock_request(&mut tx, request_id, view).await?;
        if req.requested_by != p.user_id {
            return Err(ApiError::Forbidden);
        }
        if !req.status()?.items_editable() {
            return Err(ApiError::Conflict("not_editable"));
        }
        tx.commit().await?;
    }
    ai_allowed(&state, &p, org_id).await?;
    rate_limit(&state, &p)?;
    // No transaction is held while the model runs.
    let _permit = state
        .ai_permits
        .acquire()
        .await
        .map_err(ApiError::internal)?;
    let result = extract::extract_items(state.ai.as_ref(), &text).await;
    let stored = store(
        &state,
        &p,
        NewSuggestion {
            org_id,
            capability: extract::EXTRACT_ITEMS,
            subject_type: "procurement_request",
            subject_id: request_id,
            supplier_id: None,
            text: &text,
            source: "typed",
        },
        &result,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(stored)))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ReplyFileBody {
    /// A saved supplier email (`.eml`, attachments read too), a PDF (text layer, else OCR),
    /// an Excel / CSV sheet, or a scanned page / FAX (PNG, JPEG, TIFF; OCR).
    pub file_name: String,
    /// File content, standard base64.
    pub data_base64: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ReplyText {
    /// The reply as text, for the operator to read and then pass to quotation extraction.
    pub text: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/rfqs/{rfq_id}/reply-text",
    tag = "ai",
    params(("org_id" = Uuid, Path), ("rfq_id" = Uuid, Path)),
    request_body = ReplyFileBody,
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Text read from the supplier's reply (nothing stored)", body = ReplyText),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown RFQ or not assigned", body = ErrorBody),
        (status = 422, description = "Unsupported, empty or too large file; OCR not installed", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn reply_text(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, rfq_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<ReplyFileBody>,
) -> ApiResult<Json<ReplyText>> {
    let access = p
        .require(&state, org_id, Permission::RecordQuotations)
        .await?;
    if side(access)? != Side::Operator {
        return Err(ApiError::Forbidden);
    }
    {
        let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
        let found: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM rfqs WHERE id = $1)")
            .bind(rfq_id)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        if !found {
            return Err(ApiError::NotFound);
        }
    }
    let file_name = common::required(&body.file_name, 255, false, "file_name")?;
    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        body.data_base64.as_bytes(),
    )
    .map_err(|_| ApiError::BadRequest("invalid_base64"))?;
    if bytes.is_empty() || bytes.len() > crate::storage::MAX_ATTACHMENT_BYTES {
        return Err(ApiError::Validation("file_size"));
    }
    let _permit = state
        .upload_permits
        .acquire()
        .await
        .map_err(ApiError::internal)?;
    let text = crate::intake::quote_reply_text(&file_name, &bytes)
        .await
        .map_err(|e| match e {
            crate::intake::IntakeError::Failed => ApiError::Conflict("intake_failed"),
            other => ApiError::Validation(other.code()),
        })?;
    Ok(Json(ReplyText { text }))
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/rfqs/{rfq_id}/ai/extract-quotation",
    tag = "ai",
    params(("org_id" = Uuid, Path), ("rfq_id" = Uuid, Path)),
    request_body = ExtractQuotationBody,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Suggestion stored (status pending, or failed with error_code)", body = SuggestionView),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown RFQ or not assigned", body = ErrorBody),
        (status = 409, description = "RFQ not sent, or quotations frozen", body = ErrorBody),
        (status = 422, description = "Empty or too long text, or supplier not invited", body = ErrorBody),
        (status = 429, description = "AI rate limit reached", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn extract_quotation(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, rfq_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<ExtractQuotationBody>,
) -> ApiResult<(StatusCode, Json<SuggestionView>)> {
    let access = p
        .require(&state, org_id, Permission::RecordQuotations)
        .await?;
    if side(access)? != Side::Operator {
        return Err(ApiError::Forbidden);
    }
    let text = check_text(&body.text)?;
    let (targets, currency) = {
        let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
        let (request_id, status): (Uuid, String) =
            sqlx::query_as("SELECT request_id, status FROM rfqs WHERE id = $1")
                .bind(rfq_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(ApiError::NotFound)?;
        if status != "sent" {
            return Err(ApiError::Conflict("rfq_not_sent"));
        }
        let req = lock_request(&mut tx, request_id, View::All).await?;
        use RequestStatus::*;
        if !matches!(req.status()?, RfqSent | QuotesReceived | UnderComparison) {
            return Err(ApiError::Conflict("quotations_frozen"));
        }
        let invited: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM rfq_recipients WHERE rfq_id = $1 AND supplier_id = $2)",
        )
        .bind(rfq_id)
        .bind(body.supplier_id)
        .fetch_one(&mut *tx)
        .await?;
        if !invited {
            return Err(ApiError::Validation("supplier_not_invited"));
        }
        let rows: Vec<(i32, String, Option<String>, i32)> = sqlx::query_as(
            "SELECT line_no, description, verified_mpn, quantity FROM request_items
             WHERE request_id = $1 ORDER BY line_no",
        )
        .bind(request_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let targets = rows
            .into_iter()
            .filter_map(|(line_no, description, mpn, quantity)| {
                Some(QuoteTarget {
                    line_no: u32::try_from(line_no).ok()?,
                    description,
                    mpn: mpn.unwrap_or_default(),
                    quantity: u32::try_from(quantity).ok()?,
                })
            })
            .collect::<Vec<_>>();
        (targets, req.currency()?)
    };
    ai_allowed(&state, &p, org_id).await?;
    rate_limit(&state, &p)?;
    let _permit = state
        .ai_permits
        .acquire()
        .await
        .map_err(ApiError::internal)?;
    let result = extract::extract_quotation(state.ai.as_ref(), &text, &targets, currency).await;
    let stored = store(
        &state,
        &p,
        NewSuggestion {
            org_id,
            capability: extract::EXTRACT_QUOTATION,
            subject_type: "rfq",
            subject_id: rfq_id,
            supplier_id: Some(body.supplier_id),
            text: &text,
            source: "typed",
        },
        &result,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(stored)))
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/ai/suggestions/{suggestion_id}",
    tag = "ai",
    params(("org_id" = Uuid, Path), ("suggestion_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Suggestion", body = SuggestionView),
        (status = 404, description = "Unknown or not yours", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn get_suggestion(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<SuggestionView>> {
    let access = p.access(&state, org_id).await?;
    if access == Access::None {
        return Err(ApiError::NotFound);
    }
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    // Customers see their own suggestions; assigned staff see all of this org's.
    let own_only = !matches!(access, Access::Assigned(_));
    let view = sqlx::query_as::<_, SuggestionView>(&format!(
        "SELECT {SUGGESTION_COLUMNS} FROM ai_suggestions
         WHERE id = $1 AND (NOT $2 OR requested_by = $3)"
    ))
    .bind(id)
    .bind(own_only)
    .bind(p.user_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    tx.commit().await?;
    Ok(Json(view))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct AcceptItemsBody {
    /// Positions in the suggestion's `items` array to add to the request.
    pub indices: Vec<usize>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct AcceptItemsResponse {
    pub added: usize,
}

#[derive(sqlx::FromRow)]
struct PendingSuggestion {
    capability: String,
    subject_id: Uuid,
    status: String,
    output: Option<Value>,
    requested_by: Uuid,
}

async fn lock_suggestion(tx: &mut db::Tx, id: Uuid) -> ApiResult<PendingSuggestion> {
    sqlx::query_as::<_, PendingSuggestion>(
        "SELECT capability, subject_id, status, output, requested_by FROM ai_suggestions
         WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/ai/suggestions/{suggestion_id}/accept-items",
    tag = "ai",
    params(("org_id" = Uuid, Path), ("suggestion_id" = Uuid, Path)),
    request_body = AcceptItemsBody,
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Chosen items added to the request by the requester", body = AcceptItemsResponse),
        (status = 403, description = "Not the requester", body = ErrorBody),
        (status = 404, description = "Unknown suggestion", body = ErrorBody),
        (status = 409, description = "Already decided, or request not editable", body = ErrorBody),
        (status = 422, description = "No or invalid selection, or too many items", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn accept_items(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<AcceptItemsBody>,
) -> ApiResult<Json<AcceptItemsResponse>> {
    let (access, view) = request_view(&p, &state, org_id).await?;
    require_perm(access, Permission::SubmitRequest)?;
    let mut indices = body.indices.clone();
    indices.sort_unstable();
    indices.dedup();
    if indices.is_empty() {
        return Err(ApiError::Validation("indices"));
    }
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let request_id: Uuid = sqlx::query_scalar(
        "SELECT subject_id FROM ai_suggestions WHERE id = $1 AND capability = 'extract_items'",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    let req = lock_request(&mut tx, request_id, view).await?;
    let s = lock_suggestion(&mut tx, id).await?;
    if s.requested_by != p.user_id || req.requested_by != p.user_id {
        return Err(ApiError::Forbidden);
    }
    if s.status != "pending" || s.capability != extract::EXTRACT_ITEMS || s.subject_id != req.id {
        return Err(ApiError::Conflict("already_decided"));
    }
    if !req.status()?.items_editable() {
        return Err(ApiError::Conflict("not_editable"));
    }
    let items: Vec<ItemSuggestion> = s
        .output
        .and_then(|o| serde_json::from_value(o).ok())
        .ok_or_else(|| ApiError::internal(std::io::Error::other("stored suggestion unreadable")))?;
    let chosen = indices
        .iter()
        .map(|i| items.get(*i).ok_or(ApiError::Validation("indices")))
        .collect::<ApiResult<Vec<_>>>()?;
    let existing: i64 =
        sqlx::query_scalar("SELECT count(*) FROM request_items WHERE request_id = $1")
            .bind(req.id)
            .fetch_one(&mut *tx)
            .await?;
    if existing + i64::try_from(chosen.len()).unwrap_or(i64::MAX)
        > crate::api::requests::MAX_ITEMS as i64
    {
        return Err(ApiError::Validation("too_many_items"));
    }
    let audit = Audit { org_id, p: &p };
    for item in &chosen {
        let item_id = Uuid::now_v7();
        // Same limits as items typed by hand (stored suggestions are already validated).
        sqlx::query(
            "INSERT INTO request_items (id, org_id, request_id, line_no, description, manufacturer,
                                        mpn, quantity, unit, ai_suggestion_id)
             VALUES ($1, $2, $3,
                     (SELECT coalesce(max(line_no), 0) + 1 FROM request_items WHERE request_id = $3),
                     $4, $5, $6, $7, $8, $9)",
        )
        .bind(item_id)
        .bind(org_id)
        .bind(req.id)
        .bind(&item.description)
        .bind(&item.manufacturer)
        .bind(&item.mpn)
        .bind(i32::try_from(item.quantity).map_err(|_| ApiError::Validation("quantity"))?)
        .bind(&item.unit)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        audit
            .record(
                &mut tx,
                "item.added",
                "request_item",
                item_id,
                json!({ "request": req.id, "number": req.number, "source": "ai_suggestion",
                        "suggestion": id }),
            )
            .await?;
    }
    sqlx::query(
        "UPDATE ai_suggestions SET status = 'accepted', decided_by = $2, decided_at = now(),
                decision_note = $3 WHERE id = $1",
    )
    .bind(id)
    .bind(p.user_id)
    .bind(format!("accepted {} of {}", chosen.len(), items.len()))
    .execute(&mut *tx)
    .await?;
    audit
        .record(
            &mut tx,
            "ai_suggestion.accepted",
            "ai_suggestion",
            id,
            json!({ "indices": indices, "of": items.len() }),
        )
        .await?;
    tx.commit().await?;
    Ok(Json(AcceptItemsResponse {
        added: chosen.len(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_id}/ai/suggestions/{suggestion_id}/reject",
    tag = "ai",
    params(("org_id" = Uuid, Path), ("suggestion_id" = Uuid, Path)),
    request_body = crate::api::requests::ReasonBody,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Rejected"),
        (status = 404, description = "Unknown or not yours", body = ErrorBody),
        (status = 409, description = "Already decided", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn reject(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<crate::api::requests::ReasonBody>,
) -> ApiResult<StatusCode> {
    if p.access(&state, org_id).await? == Access::None {
        return Err(ApiError::NotFound);
    }
    let reason = common::text(&body.reason, 2000, true, "reason")?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let s = lock_suggestion(&mut tx, id).await?;
    if s.requested_by != p.user_id {
        return Err(ApiError::NotFound);
    }
    if s.status != "pending" {
        return Err(ApiError::Conflict("already_decided"));
    }
    sqlx::query(
        "UPDATE ai_suggestions SET status = 'rejected', decided_by = $2, decided_at = now(),
                decision_note = $3 WHERE id = $1",
    )
    .bind(id)
    .bind(p.user_id)
    .bind(&reason)
    .execute(&mut *tx)
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "ai_suggestion.rejected",
            "ai_suggestion",
            id,
            json!({ "reason": reason }),
        )
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Used by `record_quotation`: marks the quotation suggestion the operator started from as
/// accepted, inside the same transaction. The recorded values are the operator's own.
pub async fn accept_quotation_suggestion(
    tx: &mut db::Tx,
    audit: &Audit<'_>,
    id: Uuid,
    rfq_id: Uuid,
    supplier_id: Uuid,
    quotation_id: Uuid,
) -> ApiResult<()> {
    let s = lock_suggestion(tx, id).await?;
    let supplier: Option<Uuid> =
        sqlx::query_scalar("SELECT supplier_id FROM ai_suggestions WHERE id = $1")
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
    if s.capability != extract::EXTRACT_QUOTATION
        || s.subject_id != rfq_id
        || supplier != Some(supplier_id)
    {
        return Err(ApiError::Validation("ai_suggestion_id"));
    }
    if s.status != "pending" {
        return Err(ApiError::Conflict("already_decided"));
    }
    // How many recorded lines differ from what the AI suggested (price or quantity), so
    // provenance never overstates what the AI contributed.
    let suggested: Vec<procure_ai::extract::QuoteLineSuggestion> = s
        .output
        .and_then(|o| serde_json::from_value::<procure_ai::extract::QuotationSuggestion>(o).ok())
        .map(|q| q.lines)
        .unwrap_or_default();
    let recorded: Vec<(i32, i64, i32)> = sqlx::query_as(
        "SELECT i.line_no, l.unit_price_minor, l.quantity FROM quotation_lines l
         JOIN request_items i ON i.id = l.request_item_id WHERE l.quotation_id = $1",
    )
    .bind(quotation_id)
    .fetch_all(&mut **tx)
    .await?;
    let edited = recorded
        .iter()
        .filter(|(line, price, qty)| {
            !suggested.iter().any(|sg| {
                i64::from(sg.line_no) == i64::from(*line)
                    && sg.unit_price_minor == *price
                    && i64::from(sg.quantity) == i64::from(*qty)
            })
        })
        .count();
    sqlx::query(
        "UPDATE ai_suggestions SET status = 'accepted', decided_by = $2, decided_at = now(),
                decision_note = $3 WHERE id = $1",
    )
    .bind(id)
    .bind(audit.p.user_id)
    .bind(format!(
        "recorded as quotation; {edited} of {} lines edited by the operator",
        recorded.len()
    ))
    .execute(&mut **tx)
    .await?;
    audit
        .record(
            tx,
            "ai_suggestion.accepted",
            "ai_suggestion",
            id,
            json!({ "quotation": quotation_id, "lines": recorded.len(), "edited_lines": edited }),
        )
        .await
}
