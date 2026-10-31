//! One error type for the HTTP API. Every error body has a stable machine code
//! (localised by the client) plus the request id for support correlation.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("malformed request: {0}")]
    BadRequest(&'static str),
    #[error("authentication required")]
    Unauthenticated,
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("too many requests")]
    RateLimited,
    #[error("not permitted")]
    Forbidden,
    /// Also used when the caller has no relation to the target organisation,
    /// so other tenants' existence is never revealed.
    #[error("not found")]
    NotFound,
    #[error("validation failed: {0}")]
    Validation(&'static str),
    #[error("conflict: {0}")]
    Conflict(&'static str),
    #[error("internal error")]
    Internal(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl ApiError {
    pub fn internal<E: std::error::Error + Send + Sync + 'static>(e: E) -> Self {
        ApiError::Internal(Box::new(e))
    }

    fn status_and_code(&self) -> (StatusCode, &'static str) {
        match self {
            ApiError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            ApiError::Unauthenticated => (StatusCode::UNAUTHORIZED, "unauthenticated"),
            ApiError::InvalidCredentials => (StatusCode::UNAUTHORIZED, "invalid_credentials"),
            ApiError::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
            ApiError::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            ApiError::Validation(_) => (StatusCode::UNPROCESSABLE_ENTITY, "validation_failed"),
            ApiError::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            ApiError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
        }
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        if let sqlx::Error::Database(db) = &e {
            if db.is_unique_violation() {
                return ApiError::Conflict("already_exists");
            }
            if db.is_check_violation() || db.is_foreign_key_violation() {
                return ApiError::Validation("constraint_violation");
            }
        }
        ApiError::internal(e)
    }
}

impl From<axum::extract::rejection::PathRejection> for ApiError {
    fn from(_: axum::extract::rejection::PathRejection) -> Self {
        ApiError::BadRequest("invalid_path")
    }
}

impl From<axum::extract::rejection::JsonRejection> for ApiError {
    fn from(_: axum::extract::rejection::JsonRejection) -> Self {
        ApiError::BadRequest("invalid_json")
    }
}

impl From<axum::extract::rejection::QueryRejection> for ApiError {
    fn from(_: axum::extract::rejection::QueryRejection) -> Self {
        ApiError::BadRequest("invalid_query")
    }
}

/// `Path`, `Json` and `Query` whose rejections use the standard error body.
#[derive(axum::extract::FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(ApiError))]
pub struct ApiPath<T>(pub T);

#[derive(axum::extract::FromRequest)]
#[from_request(via(axum::Json), rejection(ApiError))]
pub struct ApiJson<T>(pub T);

#[derive(axum::extract::FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(ApiError))]
pub struct ApiQuery<T>(pub T);

#[derive(Serialize)]
struct ErrorBody {
    error: ErrorInner,
}

#[derive(Serialize)]
struct ErrorInner {
    code: &'static str,
    /// Machine-readable detail (e.g. which field), never internal error text.
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'static str>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = self.status_and_code();
        if let ApiError::Internal(source) = &self {
            tracing::error!(error = %source, "internal error");
        }
        let detail = match &self {
            ApiError::Validation(d) | ApiError::Conflict(d) | ApiError::BadRequest(d) => Some(*d),
            _ => None,
        };
        (
            status,
            Json(ErrorBody {
                error: ErrorInner { code, detail },
            }),
        )
            .into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
