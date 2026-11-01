//! Chuo-Sogo Procure backend: one Axum application over PostgreSQL.

pub mod api;
pub mod audit;
pub mod db;
pub mod error;
pub mod mailer;
pub mod openapi;
pub mod principal;
pub mod ratelimit;
pub mod security;
pub mod storage;

use std::sync::Arc;
use std::time::Duration;

use axum::http::{HeaderName, StatusCode, header};
use axum::routing::{get, patch, post, put};
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
    /// Attachment files (`PROCURE_STORAGE_DIR`, default `./var/storage`).
    pub storage: Arc<storage::Storage>,
    /// Bounds concurrent attachment uploads (each holds ~25 MB while being processed).
    pub upload_permits: Arc<tokio::sync::Semaphore>,
    /// AI provider (`PROCURE_AI_PROVIDER`); the offline mock unless configured otherwise.
    pub ai: Arc<dyn procure_ai::Provider>,
    /// Bounds concurrent model calls across all users.
    pub ai_permits: Arc<tokio::sync::Semaphore>,
}

impl AppState {
    pub fn new(pool: PgPool, mailer: Arc<dyn Mailer>) -> Self {
        AppState {
            pool,
            mailer,
            limiter: Arc::new(ratelimit::RateLimiter::default()),
            hash_permits: Arc::new(tokio::sync::Semaphore::new(4)),
            storage: Arc::new(storage::Storage::new(
                std::env::var("PROCURE_STORAGE_DIR").unwrap_or_else(|_| "./var/storage".into()),
            )),
            upload_permits: Arc::new(tokio::sync::Semaphore::new(4)),
            ai: Arc::new(procure_ai::mock::MockProvider),
            ai_permits: Arc::new(tokio::sync::Semaphore::new(4)),
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
        .route("/orgs/{org_id}/audit", get(api::orgs::list_audit))
        .route(
            "/orgs/{org_id}/requests",
            get(api::requests::list_requests).post(api::requests::create_request),
        )
        .route(
            "/orgs/{org_id}/requests/{request_id}",
            get(api::requests::get_request).patch(api::requests::update_request),
        )
        .route(
            "/orgs/{org_id}/requests/{request_id}/items",
            post(api::requests::add_item),
        )
        .route(
            "/orgs/{org_id}/requests/{request_id}/items/{item_id}",
            put(api::requests::replace_item).delete(api::requests::delete_item),
        )
        .route(
            "/orgs/{org_id}/requests/{request_id}/items/{item_id}/verification",
            post(api::requests::verify_item),
        )
        .route(
            "/orgs/{org_id}/requests/{request_id}/submit",
            post(api::requests::submit),
        )
        .route(
            "/orgs/{org_id}/requests/{request_id}/cancel",
            post(api::requests::cancel),
        )
        .route(
            "/orgs/{org_id}/requests/{request_id}/pick-up",
            post(api::requests::pick_up),
        )
        .route(
            "/orgs/{org_id}/requests/{request_id}/clarification",
            post(api::requests::request_clarification),
        )
        .route(
            "/orgs/{org_id}/requests/{request_id}/start-sourcing",
            post(api::requests::start_sourcing),
        )
        .route(
            "/orgs/{org_id}/requests/{request_id}/rfqs",
            post(api::rfqs::create_rfq),
        )
        .route(
            "/orgs/{org_id}/requests/{request_id}/recommendations",
            post(api::approvals::recommend),
        )
        .route("/orgs/{org_id}/rfqs/{rfq_id}", get(api::rfqs::get_rfq))
        .route(
            "/orgs/{org_id}/rfqs/{rfq_id}/dispatch",
            post(api::rfqs::dispatch_rfq),
        )
        .route(
            "/orgs/{org_id}/rfqs/{rfq_id}/outbox",
            get(api::rfqs::rfq_outbox),
        )
        .route(
            "/orgs/{org_id}/rfqs/{rfq_id}/quotations",
            post(api::rfqs::record_quotation),
        )
        .route(
            "/orgs/{org_id}/rfqs/{rfq_id}/comparisons",
            post(api::rfqs::run_comparison),
        )
        .route(
            "/orgs/{org_id}/comparisons/{comparison_id}",
            get(api::rfqs::get_comparison),
        )
        .route(
            "/orgs/{org_id}/recommendations/{recommendation_id}",
            get(api::approvals::get_recommendation),
        )
        .route(
            "/orgs/{org_id}/recommendations/{recommendation_id}/send",
            post(api::approvals::send_recommendation),
        )
        .route(
            "/orgs/{org_id}/recommendations/{recommendation_id}/decisions",
            post(api::approvals::decide),
        )
        .route(
            "/orgs/{org_id}/recommendations/{recommendation_id}/withdraw",
            post(api::approvals::withdraw_recommendation),
        )
        .route(
            "/orgs/{org_id}/approval-policies",
            get(api::approvals::list_policies),
        )
        .route(
            "/orgs/{org_id}/approval-policies/{currency}",
            put(api::approvals::put_policy),
        )
        .route(
            "/orgs/{org_id}/suppliers",
            get(api::suppliers::list_org_suppliers),
        )
        .route(
            "/orgs/{org_id}/suppliers/{supplier_id}/proposal",
            post(api::suppliers::propose_supplier),
        )
        .route(
            "/orgs/{org_id}/suppliers/{supplier_id}/decision",
            put(api::suppliers::decide_supplier),
        )
        .route(
            "/suppliers",
            get(api::suppliers::list_suppliers).post(api::suppliers::create_supplier),
        )
        .route(
            "/suppliers/{supplier_id}/status",
            post(api::suppliers::set_supplier_status),
        )
        .route(
            "/products",
            get(api::products::list_products).post(api::products::create_product),
        )
        .route("/products/{product_id}", get(api::products::get_product))
        .route(
            "/products/{product_id}/evidence",
            post(api::products::add_evidence),
        )
        .route(
            "/products/{product_id}/status",
            post(api::products::set_product_status),
        )
        .route(
            "/orgs/{org_id}/products/{product_id}",
            get(api::products::get_org_product),
        )
        .route("/ops/queue", get(api::ops::queue))
        .route("/orgs/{org_id}/tasks", post(api::ops::create_task))
        .route(
            "/orgs/{org_id}/tasks/{task_id}",
            patch(api::ops::update_task),
        )
        .route(
            "/orgs/{org_id}/requests/{request_id}/tasks",
            get(api::ops::request_tasks),
        )
        .route(
            "/orgs/{org_id}/requests/{request_id}/comments",
            get(api::ops::list_comments).post(api::ops::add_comment),
        )
        .route(
            "/orgs/{org_id}/requests/{request_id}/communications",
            get(api::ops::list_communications).post(api::ops::log_communication),
        )
        .route(
            "/orgs/{org_id}/requests/{request_id}/time",
            get(api::ops::list_time).post(api::ops::log_time),
        )
        .route(
            "/orgs/{org_id}/attachments/{attachment_id}",
            axum::routing::delete(api::attachments::remove),
        )
        .route(
            "/orgs/{org_id}/attachments/{attachment_id}/content",
            get(api::attachments::download),
        )
        .layer(RequestBodyLimitLayer::new(MAX_JSON_BODY))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ));

    // The only route that accepts large bodies (base64 file uploads, at most 10 MB of file).
    let uploads = Router::new()
        .route(
            "/orgs/{org_id}/requests/{request_id}/attachments",
            get(api::attachments::list).post(api::attachments::upload),
        )
        .layer(axum::extract::DefaultBodyLimit::max(
            api::attachments::MAX_UPLOAD_BODY,
        ))
        .layer(RequestBodyLimitLayer::new(
            api::attachments::MAX_UPLOAD_BODY,
        ))
        // 10 MB over a slow uplink needs more than the default 30 s.
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(180),
        ));

    // Model calls can take minutes (with retries); only the two extraction routes get that long.
    let ai = Router::new()
        .route(
            "/orgs/{org_id}/requests/{request_id}/ai/extract-items",
            post(api::ai::extract_items),
        )
        .route(
            "/orgs/{org_id}/rfqs/{rfq_id}/ai/extract-quotation",
            post(api::ai::extract_quotation),
        )
        .layer(RequestBodyLimitLayer::new(MAX_JSON_BODY))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(300),
        ));
    let ai_review = Router::new()
        .route(
            "/orgs/{org_id}/ai/suggestions/{suggestion_id}",
            get(api::ai::get_suggestion),
        )
        .route(
            "/orgs/{org_id}/ai/suggestions/{suggestion_id}/accept-items",
            post(api::ai::accept_items),
        )
        .route(
            "/orgs/{org_id}/ai/suggestions/{suggestion_id}/reject",
            post(api::ai::reject),
        )
        .layer(RequestBodyLimitLayer::new(MAX_JSON_BODY))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ));

    Router::new()
        .route(
            "/health",
            get(|| async { Json(serde_json::json!({ "status": "ok" })) }),
        )
        .nest("/api/v1", api.merge(uploads).merge(ai).merge(ai_review))
        .with_state(state)
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
