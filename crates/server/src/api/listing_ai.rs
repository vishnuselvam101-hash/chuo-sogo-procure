//! AI help for supplier listings (Phase D): a voice note, a catalogue file or typed notes
//! become a listing suggestion; title and description can be drafted in the other language.
//!
//! Everything here produces an `ai_suggestions` row only. The supplier applies a suggestion
//! to the editor and saves it like any typed change (which takes the listing back to draft),
//! and Chuo-Sogo reviews the listing before it is published: AI output never reaches the
//! catalogue unseen.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use base64::Engine;
use procure_ai::listing as ai_listing;
use procure_domain::rbac::{Access, Permission};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::api::ai::{
    NewSuggestion, SUGGESTION_COLUMNS, SuggestionView, ai_allowed, check_text, rate_limit, store,
};
use crate::api::common::{self, Audit};
use crate::db;
use crate::error::{ApiError, ApiJson, ApiPath, ApiResult, ErrorBody};
use crate::intake::{self, IntakeError};
use crate::principal::Principal;
use crate::storage::MAX_ATTACHMENT_BYTES;

#[derive(Serialize, utoipa::ToSchema)]
pub struct ListingSuggestionView {
    #[serde(flatten)]
    pub suggestion: SuggestionView,
    /// The text the suggestion was made from (typed, transcribed or read from the file), so
    /// the supplier can check what was heard or read.
    pub input_text: String,
    /// `typed`, `voice` or `file`.
    pub input_source: String,
}

async fn supplier_access(state: &AppState, p: &Principal, org_id: Uuid) -> ApiResult<()> {
    let access = p.access(state, org_id).await?;
    if !matches!(access, Access::Member(_)) {
        return Err(ApiError::NotFound);
    }
    common::require_perm(access, Permission::ManageListings)
}

/// The listing the suggestion is for, if any, must be the supplier's own.
async fn check_listing(
    state: &AppState,
    p: &Principal,
    org_id: Uuid,
    listing_id: Option<Uuid>,
) -> ApiResult<()> {
    let Some(id) = listing_id else {
        return Ok(());
    };
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let ok: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM product_listings WHERE id = $1 AND org_id = $2 AND status <> 'retired')",
    )
    .bind(id)
    .bind(org_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    if ok { Ok(()) } else { Err(ApiError::NotFound) }
}

fn intake_error(e: IntakeError) -> ApiError {
    match e {
        IntakeError::SpeechUnavailable => ApiError::Conflict("speech_unavailable"),
        IntakeError::Failed => ApiError::Conflict("intake_failed"),
        other => ApiError::Validation(other.code()),
    }
}

/// Runs the extraction on `text`, stores the suggestion, returns it with its input.
async fn suggest(
    state: &AppState,
    p: &Principal,
    org_id: Uuid,
    listing_id: Option<Uuid>,
    text: &str,
    source: &'static str,
) -> ApiResult<ListingSuggestionView> {
    let _permit = state
        .ai_permits
        .acquire()
        .await
        .map_err(ApiError::internal)?;
    let result = ai_listing::extract_listing(state.ai.as_ref(), text).await;
    let view = store(
        state,
        p,
        NewSuggestion {
            org_id,
            capability: ai_listing::EXTRACT_LISTING,
            subject_type: if listing_id.is_some() {
                "listing"
            } else {
                "supplier_org"
            },
            subject_id: listing_id.unwrap_or(org_id),
            supplier_id: None,
            text,
            source,
        },
        &result,
    )
    .await?;
    Ok(ListingSuggestionView {
        suggestion: view,
        input_text: text.to_string(),
        input_source: source.to_string(),
    })
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct FromTextBody {
    pub text: String,
    /// The listing being edited (none: a new listing).
    pub listing_id: Option<Uuid>,
}

#[utoipa::path(
    operation_id = "listing_ai_from_text",
    post,
    path = "/api/v1/orgs/{org_id}/listings/ai/from-text",
    tag = "listings",
    params(("org_id" = Uuid, Path)),
    request_body = FromTextBody,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Suggestion (or a failed attempt) recorded", body = ListingSuggestionView),
        (status = 409, description = "AI switched off for this organisation", body = ErrorBody),
        (status = 422, description = "Empty or too long", body = ErrorBody),
        (status = 429, description = "Too many AI requests", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation or listing", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn from_text(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<FromTextBody>,
) -> ApiResult<(StatusCode, Json<ListingSuggestionView>)> {
    supplier_access(&state, &p, org_id).await?;
    let text = check_text(&body.text)?;
    check_listing(&state, &p, org_id, body.listing_id).await?;
    ai_allowed(&state, &p, org_id).await?;
    rate_limit(&state, &p)?;
    let view = suggest(&state, &p, org_id, body.listing_id, &text, "typed").await?;
    Ok((StatusCode::CREATED, Json(view)))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct FileBody {
    pub file_name: String,
    /// File content, standard base64.
    pub data_base64: String,
    /// Voice notes: `ja`, `en`, `ta` or `hi`; empty = detect.
    pub language: Option<String>,
    pub listing_id: Option<Uuid>,
}

fn decode(body: &FileBody) -> ApiResult<Vec<u8>> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body.data_base64.as_bytes())
        .map_err(|_| ApiError::BadRequest("invalid_base64"))?;
    if bytes.is_empty() || bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(ApiError::Validation("file_size"));
    }
    Ok(bytes)
}

#[utoipa::path(
    operation_id = "listing_ai_from_voice",
    post,
    path = "/api/v1/orgs/{org_id}/listings/ai/from-voice",
    tag = "listings",
    params(("org_id" = Uuid, Path)),
    request_body = FileBody,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Transcribed on this server; suggestion recorded (the audio itself is not kept)", body = ListingSuggestionView),
        (status = 409, description = "Speech-to-text not available on this server, or AI switched off", body = ErrorBody),
        (status = 422, description = "Not audio, longer than 5 minutes, or nothing heard", body = ErrorBody),
        (status = 413, description = "Larger than 10 MB"),
        (status = 429, description = "Too many AI requests", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation or listing", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn from_voice(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<FileBody>,
) -> ApiResult<(StatusCode, Json<ListingSuggestionView>)> {
    supplier_access(&state, &p, org_id).await?;
    let language = match body.language.as_deref().unwrap_or("") {
        "" => None,
        l @ ("ja" | "en" | "ta" | "hi") => Some(l.to_string()),
        _ => return Err(ApiError::Validation("language")),
    };
    check_listing(&state, &p, org_id, body.listing_id).await?;
    ai_allowed(&state, &p, org_id).await?;
    let transcriber = state
        .transcriber
        .clone()
        .ok_or(ApiError::Conflict("speech_unavailable"))?;
    rate_limit(&state, &p)?;
    let audio = decode(&body)?;
    let kind = intake::audio_kind(&audio).ok_or(ApiError::Validation("file_type"))?;
    let transcript = {
        let _permit = state
            .upload_permits
            .acquire()
            .await
            .map_err(ApiError::internal)?;
        transcriber
            .transcribe(&audio, kind, language.as_deref())
            .await
            .map_err(intake_error)?
    };
    drop(audio);
    let view = suggest(&state, &p, org_id, body.listing_id, &transcript, "voice").await?;
    Ok((StatusCode::CREATED, Json(view)))
}

#[utoipa::path(
    operation_id = "listing_ai_from_file",
    post,
    path = "/api/v1/orgs/{org_id}/listings/ai/from-file",
    tag = "listings",
    params(("org_id" = Uuid, Path)),
    request_body = FileBody,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Text read from the file; suggestion recorded", body = ListingSuggestionView),
        (status = 409, description = "AI switched off, or the file could not be read", body = ErrorBody),
        (status = 422, description = "Not a PDF/Excel/CSV/text file, or no text in it (scanned PDF)", body = ErrorBody),
        (status = 413, description = "Larger than 10 MB"),
        (status = 429, description = "Too many AI requests", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation or listing", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn from_file(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<FileBody>,
) -> ApiResult<(StatusCode, Json<ListingSuggestionView>)> {
    supplier_access(&state, &p, org_id).await?;
    check_listing(&state, &p, org_id, body.listing_id).await?;
    ai_allowed(&state, &p, org_id).await?;
    rate_limit(&state, &p)?;
    let bytes = decode(&body)?;
    let text = {
        let _permit = state
            .upload_permits
            .acquire()
            .await
            .map_err(ApiError::internal)?;
        intake::document_text(&body.file_name, &bytes)
            .await
            .map_err(intake_error)?
    };
    let view = suggest(&state, &p, org_id, body.listing_id, &text, "file").await?;
    Ok((StatusCode::CREATED, Json(view)))
}

#[utoipa::path(
    operation_id = "listing_ai_translate",
    post,
    path = "/api/v1/orgs/{org_id}/listings/{id}/ai/translate",
    tag = "listings",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Draft of title and description in the other language", body = ListingSuggestionView),
        (status = 422, description = "Nothing to translate", body = ErrorBody),
        (status = 409, description = "AI switched off", body = ErrorBody),
        (status = 429, description = "Too many AI requests", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown listing", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn translate(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<(StatusCode, Json<ListingSuggestionView>)> {
    supplier_access(&state, &p, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let row: Option<(String, String, String, String)> = sqlx::query_as(
        "SELECT title, description, title_en, description_en FROM product_listings
         WHERE id = $1 AND org_id = $2 AND status <> 'retired'",
    )
    .bind(id)
    .bind(org_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    let (title, description, title_en, description_en) = row.ok_or(ApiError::NotFound)?;
    // Translate from whichever language the supplier wrote.
    let (from_title, from_description) = if !title.trim().is_empty() {
        (title, description)
    } else {
        (title_en, description_en)
    };
    if from_title.trim().is_empty() && from_description.trim().is_empty() {
        return Err(ApiError::Validation("text"));
    }
    ai_allowed(&state, &p, org_id).await?;
    rate_limit(&state, &p)?;
    let input = format!("{from_title}\n{from_description}");
    let result = {
        let _permit = state
            .ai_permits
            .acquire()
            .await
            .map_err(ApiError::internal)?;
        ai_listing::translate_listing(state.ai.as_ref(), &from_title, &from_description).await
    };
    let view = store(
        &state,
        &p,
        NewSuggestion {
            org_id,
            capability: ai_listing::TRANSLATE_LISTING,
            subject_type: "listing",
            subject_id: id,
            supplier_id: None,
            text: &input,
            source: "typed",
        },
        &result,
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(ListingSuggestionView {
            suggestion: view,
            input_text: input,
            input_source: "typed".into(),
        }),
    ))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct DecisionBody {
    /// true: applied to the editor; false: discarded.
    pub accepted: bool,
    #[serde(default)]
    pub note: String,
}

#[utoipa::path(
    operation_id = "listing_ai_decide",
    post,
    path = "/api/v1/orgs/{org_id}/listings/ai/suggestions/{suggestion_id}/decision",
    tag = "listings",
    params(("org_id" = Uuid, Path), ("suggestion_id" = Uuid, Path)),
    request_body = DecisionBody,
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Decision recorded (the supplier still saves the listing itself)", body = SuggestionView),
        (status = 409, description = "Already decided or failed", body = ErrorBody),
        (status = 404, description = "Unknown suggestion", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn decide(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, sid)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<DecisionBody>,
) -> ApiResult<Json<SuggestionView>> {
    supplier_access(&state, &p, org_id).await?;
    let note = common::text(&body.note, 2000, true, "note")?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT status, capability FROM ai_suggestions WHERE id = $1 AND org_id = $2 FOR UPDATE",
    )
    .bind(sid)
    .bind(org_id)
    .fetch_optional(&mut *tx)
    .await?;
    let (status, capability) = row.ok_or(ApiError::NotFound)?;
    if !matches!(
        capability.as_str(),
        ai_listing::EXTRACT_LISTING | ai_listing::TRANSLATE_LISTING
    ) {
        return Err(ApiError::NotFound);
    }
    if status != "pending" {
        return Err(ApiError::Conflict("already_decided"));
    }
    let to = if body.accepted {
        "accepted"
    } else {
        "rejected"
    };
    let view = sqlx::query_as::<_, SuggestionView>(&format!(
        "UPDATE ai_suggestions SET status = $2, decided_by = $3, decided_at = now(), decision_note = $4
         WHERE id = $1 RETURNING {SUGGESTION_COLUMNS}"
    ))
    .bind(sid)
    .bind(to)
    .bind(p.user_id)
    .bind(&note)
    .fetch_one(&mut *tx)
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            if body.accepted {
                "ai.suggestion_applied"
            } else {
                "ai.suggestion_rejected"
            },
            "ai_suggestion",
            sid,
            json!({ "capability": capability }),
        )
        .await?;
    tx.commit().await?;
    Ok(Json(view))
}
