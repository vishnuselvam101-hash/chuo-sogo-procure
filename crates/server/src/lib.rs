//! Chuo-Sogo Procure backend: one Axum application over PostgreSQL.

pub mod api;
pub mod audit;
pub mod db;
pub mod error;
pub mod mailer;
pub mod principal;
pub mod ratelimit;
pub mod security;

use std::sync::Arc;
use std::time::Duration;

use axum::http::{HeaderName, StatusCode, header};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use sqlx::PgPool;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::Level;
use uuid::Uuid;

use crate::mailer::Mailer;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub mailer: Arc<dyn Mailer>,
    pub limiter: Arc<ratelimit::RateLimiter>,
    /// Bounds concurrent argon2 work (CPU + ~19 MiB each).
    pub hash_permits: Arc<tokio::sync::Semaphore>,
}

impl AppState {
    pub fn new(pool: PgPool, mailer: Arc<dyn Mailer>) -> Self {
        AppState {
            pool,
            mailer,
            limiter: Arc::new(ratelimit::RateLimiter::default()),
            hash_permits: Arc::new(tokio::sync::Semaphore::new(4)),
        }
    }
}

/// Keeps a client-supplied `x-request-id` only if it is a UUID; otherwise drops it so the
/// server generates one. Request ids end up in audit records and must not be arbitrary text.
async fn sanitise_request_id(
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let valid = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| Uuid::parse_str(v).is_ok());
    if !valid {
        req.headers_mut().remove("x-request-id");
    }
    next.run(req).await
}

const MAX_JSON_BODY: usize = 1024 * 1024;

pub fn router(state: AppState) -> Router {
    let request_id = HeaderName::from_static("x-request-id");
    let api = Router::new()
        .route("/auth/login", post(api::auth::login))
        .route("/auth/logout", post(api::auth::logout))
        .route(
            "/auth/password-reset",
            post(api::auth::request_password_reset),
        )
        .route("/auth/set-password", post(api::auth::set_password))
        .route("/me", get(api::auth::me))
        .route(
            "/invitations/{org_id}/{answer}",
            post(api::auth::respond_invitation),
        )
        .route(
            "/platform/organisations",
            get(api::orgs::list_organisations).post(api::orgs::create_organisation),
        )
        .route(
            "/orgs/{org_id}/members",
            get(api::orgs::list_members).post(api::orgs::add_member),
        )
        .route(
            "/orgs/{org_id}/members/{user_id}",
            patch(api::orgs::update_member),
        )
        .route(
            "/orgs/{org_id}/assignments",
            post(api::orgs::assign_operator),
        )
        .route(
            "/orgs/{org_id}/assignments/{user_id}",
            axum::routing::delete(api::orgs::revoke_operator),
        )
        .route("/orgs/{org_id}/audit", get(api::orgs::list_audit));

    Router::new()
        .route(
            "/health",
            get(|| async { Json(serde_json::json!({ "status": "ok" })) }),
        )
        .nest("/api/v1", api)
        .with_state(state)
        .layer(RequestBodyLimitLayer::new(MAX_JSON_BODY))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(
            TraceLayer::new_for_http()
                // Path only (no query string), plus the correlation id set by the outer layer.
                .make_span_with(|req: &axum::http::Request<_>| {
                    let request_id = req.headers().get("x-request-id").and_then(|v| v.to_str().ok()).unwrap_or("");
                    tracing::info_span!("http", method = %req.method(), path = %req.uri().path(), request_id)
                })
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(SetSensitiveRequestHeadersLayer::new([
            header::AUTHORIZATION,
            header::COOKIE,
        ]))
        .layer(PropagateRequestIdLayer::new(request_id.clone()))
        .layer(SetRequestIdLayer::new(request_id, MakeRequestUuid))
        .layer(axum::middleware::from_fn(sanitise_request_id))
}

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("a platform organisation already exists")]
    AlreadyBootstrapped,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Api(#[from] error::ApiError),
}

/// Creates the single platform organisation and its first super admin.
/// Returns the one-time activation token (shown once, never stored in clear).
pub async fn bootstrap(
    pool: &PgPool,
    platform_name: &str,
    email: &str,
    name: &str,
) -> Result<String, BootstrapError> {
    let mut tx = db::auth_tx(pool).await?;
    db::enable_platform_scope(&mut tx).await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM organisations WHERE kind = 'platform')")
            .fetch_one(&mut *tx)
            .await?;
    if exists {
        return Err(BootstrapError::AlreadyBootstrapped);
    }
    let org_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();
    sqlx::query("INSERT INTO organisations (id, kind, name) VALUES ($1, 'platform', $2)")
        .bind(org_id)
        .bind(platform_name)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO users (id, email, display_name, account_kind) VALUES ($1, $2, $3, 'internal')",
    )
    .bind(user_id)
    .bind(email)
    .bind(name)
    .execute(&mut *tx)
    .await?;
    db::set_org(&mut tx, org_id).await?;
    sqlx::query(
        "INSERT INTO memberships (user_id, org_id, role) VALUES ($1, $2, 'platform_super_admin')",
    )
    .bind(user_id)
    .bind(org_id)
    .execute(&mut *tx)
    .await?;
    audit::record(
        &mut tx,
        audit::Event {
            org_id,
            actor: None,
            action: "platform.bootstrapped",
            entity_type: "organisation",
            entity_id: Some(org_id.to_string()),
            data: serde_json::json!({ "admin_user": user_id }),
            request_id: None,
        },
    )
    .await?;
    let token = api::auth::issue_token(
        &mut tx,
        user_id,
        "activation",
        api::auth::ACTIVATION_TOKEN_TTL,
    )
    .await?;
    tx.commit().await?;
    Ok(token)
}
