//! Chuo-Sogo Procure backend: one Axum application over PostgreSQL.

pub mod agents;
pub mod api;
pub mod audit;
pub mod db;
pub mod error;
pub mod intake;
pub mod integrations;
pub mod invoice_pdf;
pub mod mailer;
pub mod media;
pub mod mfa;
pub mod openapi;
pub mod principal;
pub mod ratelimit;
pub mod security;
pub mod storage;
pub mod supplier_mail;
pub mod verify;

use std::sync::Arc;
use std::time::Duration;

use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use sqlx::PgPool;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;
use tower_http::set_header::SetResponseHeaderLayer;
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
    /// Encrypts admin second-factor secrets (`PROCURE_SECRET_KEY`); admin sign-in is
    /// unavailable without it.
    pub secret_box: Option<Arc<mfa::SecretBox>>,
    /// Networks allowed to reach the admin API (`PROCURE_ADMIN_ALLOWED_CIDRS`, default this
    /// machine only). `None` only in in-process tests.
    pub admin_networks: Option<Vec<ipnet::IpNet>>,
    /// Font for invoice PDFs (`PROCURE_PDF_FONT`); documents cannot be issued without it.
    pub pdf_font: Option<invoice_pdf::PdfFont>,
    /// Speech-to-text for voice notes (`PROCURE_STT_COMMAND` + `PROCURE_STT_MODEL`); voice
    /// input is unavailable without it.
    pub transcriber: Option<Arc<dyn intake::Transcriber>>,
    /// Total bytes of uploaded files one organisation may keep (`PROCURE_ORG_STORAGE_QUOTA_MB`);
    /// unlimited when unset. Set on the public demo so one account cannot fill the disk.
    pub org_storage_quota: Option<i64>,
    /// Consumer self sign-up in the shop (`PROCURE_SHOP_SIGNUP=off` turns it off outside the
    /// admin networks, e.g. on the public demo, where nobody can receive the activation mail).
    pub shop_signup: bool,
    /// Trial server only (`PROCURE_DEMO_LOGIN`, comma-separated emails): these accounts can be
    /// entered without a password. Empty, i.e. off, everywhere else.
    pub demo_accounts: Vec<String>,
    /// Live supplier mail and FAX (`PROCURE_SUPPLIER_MAIL=live`); `None` = test mode, every
    /// supplier message is only recorded.
    pub supplier_mail: Option<Arc<supplier_mail::SupplierMail>>,
    /// The agents this server runs (`agents::registry()`); tests install their own.
    pub agents: Arc<Vec<Arc<dyn agents::Agent>>>,
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
            secret_box: None,
            admin_networks: Some(admin_networks_from(
                &std::env::var("PROCURE_ADMIN_ALLOWED_CIDRS")
                    .unwrap_or_else(|_| "127.0.0.1/32,::1/128".into()),
            )),
            pdf_font: None,
            transcriber: None,
            org_storage_quota: std::env::var("PROCURE_ORG_STORAGE_QUOTA_MB")
                .ok()
                .and_then(|v| v.trim().parse::<i64>().ok())
                .filter(|mb| *mb > 0)
                .map(|mb| mb.saturating_mul(1024 * 1024)),
            shop_signup: std::env::var("PROCURE_SHOP_SIGNUP").as_deref() != Ok("off"),
            demo_accounts: std::env::var("PROCURE_DEMO_LOGIN")
                .unwrap_or_default()
                .split(',')
                .map(|e| e.trim().to_lowercase())
                .filter(|e| !e.is_empty())
                .collect(),
            supplier_mail: None,
            agents: Arc::new(agents::registry()),
        }
    }
}

/// Parses a comma-separated list of networks; invalid entries are ignored (logged), so a typo
/// can only make the allow-list stricter.
pub fn admin_networks_from(list: &str) -> Vec<ipnet::IpNet> {
    list.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| match s.parse::<ipnet::IpNet>() {
            Ok(n) => Some(n),
            Err(_) => {
                tracing::warn!(entry = %s, "ignoring invalid PROCURE_ADMIN_ALLOWED_CIDRS entry");
                None
            }
        })
        .collect()
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
        .route("/auth/demo-login", post(api::demo::login))
        .route("/demo/accounts", get(api::demo::accounts))
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
        .route("/orgs/{org_id}/purchase-history", get(api::history::search))
        .route("/orgs/{org_id}/records", get(api::records::search_records))
        .route("/mail/status", get(api::mail::mail_status))
        .route(
            "/suppliers/{supplier_id}/contact",
            put(api::suppliers::set_contact),
        )
        .route("/mail-inbox", get(api::mail::list_inbox))
        .route("/mail-inbox/{message_id}/text", get(api::mail::inbox_text))
        .route("/mail-inbox/{message_id}/assign", post(api::mail::assign))
        .route(
            "/mail-inbox/{message_id}/set-aside",
            post(api::mail::set_aside),
        )
        .route(
            "/orgs/{org_id}/rfqs/{rfq_id}/remind",
            post(api::rfqs::remind_rfq),
        )
        .route(
            "/orgs/{org_id}/rfqs/{rfq_id}/supplier-documents/{document_id}/text",
            post(api::ai::document_text),
        )
        .route(
            "/orgs/{org_id}/supplier-documents/{document_id}/content",
            get(api::records::download_document),
        )
        .route(
            "/orgs/{org_id}/purchase-history/opportunities",
            get(api::history::opportunities),
        )
        .route(
            "/orgs/{org_id}/purchase-history/imports/{id}/withdraw",
            post(api::history::withdraw),
        )
        .route(
            "/orgs/{org_id}/requests/{request_id}/price-reference",
            get(api::history::price_reference),
        )
        .route(
            "/orgs/{org_id}/buying-criteria",
            get(api::history::get_criteria).put(api::history::put_criteria),
        )
        .route(
            "/orgs/{org_id}/item-aliases",
            get(api::history::list_aliases).post(api::history::create_alias),
        )
        .route(
            "/orgs/{org_id}/item-aliases/{id}",
            axum::routing::delete(api::history::delete_alias),
        )
        .route(
            "/orgs/{org_id}/requests/{request_id}/items/{item_id}/similar-items",
            get(api::history::similar_items),
        )
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
            "/orgs/{org_id}/requests/{request_id}/supplier-shortlist",
            get(api::shortlist::supplier_shortlist),
        )
        .route(
            "/orgs/{org_id}/requests/{request_id}/recommendations",
            post(api::approvals::recommend),
        )
        .route("/orgs/{org_id}/rfqs/{rfq_id}", get(api::rfqs::get_rfq))
        .route(
            "/orgs/{org_id}/rfqs/{rfq_id}/check",
            post(api::rfqs::check_rfq),
        )
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
            "/orgs/{org_id}/recommendations/{recommendation_id}/check",
            post(api::approvals::check_recommendation),
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
        .route(
            "/orgs/{org_id}/requests/{request_id}/order",
            get(api::orders::get_order).post(api::orders::record_order),
        )
        .route(
            "/orgs/{org_id}/orders/{order_id}/deliveries",
            post(api::orders::record_delivery),
        )
        .route(
            "/orgs/{org_id}/deliveries/{delivery_id}/reverse",
            post(api::orders::reverse_delivery),
        )
        .route(
            "/orgs/{org_id}/orders/{order_id}/invoice",
            post(api::orders::record_invoice),
        )
        .route(
            "/orgs/{org_id}/orders/{order_id}/delivery-date",
            post(api::orders::confirm_delivery_date),
        )
        .route(
            "/orgs/{org_id}/orders/{order_id}/order-email",
            get(api::order_email::get_order_email),
        )
        .route(
            "/orgs/{org_id}/orders/{order_id}/order-email/send",
            post(api::order_email::send_order_email),
        )
        .route(
            "/orgs/{org_id}/exports/orders.csv",
            get(api::exports::orders_csv),
        )
        .route(
            "/orgs/{org_id}/exports/savings.csv",
            get(api::exports::savings_csv),
        )
        .route(
            "/orgs/{org_id}/requests/import",
            post(api::requests::import_requests),
        )
        .route(
            "/orgs/{org_id}/discrepancies/{discrepancy_id}/resolve",
            post(api::orders::resolve_discrepancy),
        )
        .route(
            "/orgs/{org_id}/billing/invoices",
            get(api::billing::customer_list),
        )
        .route("/orgs/{org_id}/savings", get(api::savings::get_savings))
        .route(
            "/orgs/{org_id}/savings/baselines/{baseline_id}/confirm",
            post(api::savings::confirm_baseline),
        )
        .route(
            "/orgs/{org_id}/listings",
            get(api::listings::list_own).post(api::listings::create),
        )
        .route(
            "/orgs/{org_id}/listings/{id}",
            get(api::listings::get_own)
                .put(api::listings::update)
                .delete(api::listings::delete_draft),
        )
        .route(
            "/orgs/{org_id}/listings/{id}/submit",
            post(api::listings::submit),
        )
        .route(
            "/orgs/{org_id}/listings/{id}/retire",
            post(api::listings::retire_own),
        )
        .route(
            "/orgs/{org_id}/listings/{id}/media/{media_id}",
            axum::routing::delete(api::listings::delete_media),
        )
        .route(
            "/orgs/{org_id}/listings/{id}/media/{media_id}/content",
            get(api::listings::own_media),
        )
        .route("/shop/info", get(api::shop::info))
        .route("/shop/signup", post(api::shop::signup))
        .route("/orgs/{org_id}/shop/products", get(api::shop::products))
        .route("/orgs/{org_id}/shop/products/{id}", get(api::shop::product))
        .route(
            "/orgs/{org_id}/shop/products/{id}/media/{media_id}/content",
            get(api::shop::product_media),
        )
        .route(
            "/orgs/{org_id}/shop/orders",
            get(api::shop::my_orders).post(api::shop::place_order),
        )
        .route("/orgs/{org_id}/shop/orders/{id}", get(api::shop::my_order))
        .route(
            "/orgs/{org_id}/shop/orders/{id}/cancel",
            post(api::shop::cancel_mine),
        )
        .route("/orgs/{org_id}/shop/my-data", get(api::shop::my_data))
        .route(
            "/orgs/{org_id}/shop/close-account",
            post(api::shop::close_account),
        )
        .route("/orgs/{org_id}/catalogue", get(api::listings::catalogue))
        .route(
            "/orgs/{org_id}/catalogue/{id}",
            get(api::listings::catalogue_item),
        )
        .route(
            "/orgs/{org_id}/catalogue/{id}/media/{media_id}/content",
            get(api::listings::catalogue_media),
        )
        .route(
            "/orgs/{org_id}/billing/invoices/{id}",
            get(api::billing::customer_get),
        )
        .route(
            "/orgs/{org_id}/billing/invoices/{id}/pdf",
            get(api::billing::customer_pdf),
        )
        .route(
            "/orgs/{org_id}/requests/{request_id}/close",
            post(api::orders::close_request),
        )
        .route(
            "/orgs/{org_id}/agents/settings",
            get(api::agents::get_settings).put(api::agents::put_settings),
        )
        .route("/orgs/{org_id}/agents/runs", get(api::agents::list_runs))
        .route(
            "/orgs/{org_id}/agents/{agent}/run",
            post(api::agents::trigger_run),
        )
        .route(
            "/orgs/{org_id}/agents/exceptions",
            get(api::agents::list_exceptions),
        )
        .route(
            "/orgs/{org_id}/agents/exceptions/{exception_id}/decide",
            post(api::agents::decide),
        )
        .route("/orgs/{org_id}/agents/summary", get(api::agents::summary))
        .route(
            "/orgs/{org_id}/matching/tolerances",
            get(api::matching::get_tolerances).put(api::matching::put_tolerances),
        )
        .route(
            "/orgs/{org_id}/orders/{order_id}/ai/extract-invoice",
            post(api::matching::extract_invoice),
        )
        .route(
            "/orgs/{org_id}/cost/models",
            get(api::cost::list_models).post(api::cost::create_model),
        )
        .route(
            "/orgs/{org_id}/cost/models/{model_id}",
            get(api::cost::get_model),
        )
        .route(
            "/orgs/{org_id}/cost/quotations/{quotation_id}/facts",
            get(api::cost::get_quotation_facts),
        )
        .route(
            "/orgs/{org_id}/cost/briefs",
            get(api::cost::list_briefs).post(api::cost::create_brief),
        )
        .route(
            "/orgs/{org_id}/cost/briefs/{brief_id}",
            get(api::cost::get_brief),
        )
        .route(
            "/orgs/{org_id}/cost/briefs/{brief_id}/narrative",
            post(api::cost::draft_narrative),
        )
        .route(
            "/orgs/{org_id}/cost/briefs/{brief_id}/finalise",
            post(api::cost::finalise_brief),
        )
        .route(
            "/orgs/{org_id}/agents/chasing/settings",
            get(api::chasing::get_settings).put(api::chasing::put_settings),
        )
        .route(
            "/suppliers/{supplier_id}/compliance-docs/{doc_id}",
            put(api::supplier_watch::update_doc),
        )
        .route(
            "/suppliers/{supplier_id}/compliance-docs/{doc_id}/verify",
            post(api::supplier_watch::verify_doc),
        )
        .route(
            "/suppliers/{supplier_id}/compliance-docs/{doc_id}/file",
            get(api::supplier_watch::doc_file),
        )
        .route(
            "/orgs/{org_id}/suppliers/{supplier_id}/compliance-docs",
            get(api::supplier_watch::org_docs),
        )
        .route(
            "/orgs/{org_id}/supplier-scorecards",
            get(api::supplier_watch::scorecards),
        )
        .route(
            "/orgs/{org_id}/integrations/profiles",
            get(api::integrations::list_profiles),
        )
        .route(
            "/orgs/{org_id}/integrations/profiles/{name}",
            axum::routing::put(api::integrations::put_profile)
                .delete(api::integrations::delete_profile),
        )
        .route(
            "/orgs/{org_id}/integrations/imports/{batch_id}",
            get(api::integrations::get_import),
        )
        .route(
            "/orgs/{org_id}/integrations/imports/{batch_id}/apply",
            post(api::integrations::apply),
        )
        .route(
            "/orgs/{org_id}/integrations/imports/{batch_id}/discard",
            post(api::integrations::discard),
        )
        .route(
            "/orgs/{org_id}/integrations/exports/agent-results.csv",
            get(api::integrations::export_agent_results),
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
        .route(
            "/orgs/{org_id}/listings/{id}/media",
            post(api::listings::upload_media),
        )
        .route(
            "/orgs/{org_id}/purchase-history/preview",
            post(api::history::preview),
        )
        .route(
            "/orgs/{org_id}/rfqs/{rfq_id}/reply-text",
            post(api::ai::reply_text),
        )
        .route(
            "/orgs/{org_id}/purchase-history/imports",
            get(api::history::list_imports).post(api::history::import),
        )
        .route(
            "/suppliers/{supplier_id}/compliance-docs",
            get(api::supplier_watch::list_docs).post(api::supplier_watch::create_doc),
        )
        .route(
            "/orgs/{org_id}/integrations/imports",
            get(api::integrations::list_imports).post(api::integrations::upload),
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
        .route(
            "/orgs/{org_id}/listings/ai/from-text",
            post(api::listing_ai::from_text),
        )
        .route(
            "/orgs/{org_id}/listings/{id}/ai/translate",
            post(api::listing_ai::translate),
        )
        .layer(RequestBodyLimitLayer::new(MAX_JSON_BODY))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(300),
        ));
    // Voice notes and catalogue files for the listing assistant: upload-sized bodies, and
    // time for transcription plus the model.
    let ai_uploads = Router::new()
        .route(
            "/orgs/{org_id}/listings/ai/from-voice",
            post(api::listing_ai::from_voice),
        )
        .route(
            "/orgs/{org_id}/listings/ai/from-file",
            post(api::listing_ai::from_file),
        )
        .layer(axum::extract::DefaultBodyLimit::max(
            api::attachments::MAX_UPLOAD_BODY,
        ))
        .layer(RequestBodyLimitLayer::new(
            api::attachments::MAX_UPLOAD_BODY,
        ))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(600),
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
        .route(
            "/orgs/{org_id}/listings/ai/suggestions/{suggestion_id}/decision",
            post(api::listing_ai::decide),
        )
        .layer(RequestBodyLimitLayer::new(MAX_JSON_BODY))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ));

    // Admin console API: separate namespace, sessions only valid here.
    let admin = Router::new()
        .route("/auth/login", post(api::admin::login))
        .route("/auth/mfa", post(api::admin::verify_mfa))
        .route("/auth/totp/enroll", post(api::admin::enroll))
        .route("/auth/totp/confirm", post(api::admin::confirm_enrollment))
        .route("/auth/logout", post(api::admin::logout))
        .route("/me", get(api::admin::me))
        .route("/health", get(api::admin::health))
        .route(
            "/orgs",
            get(api::admin::list_orgs).post(api::admin::create_org),
        )
        .route("/orgs/{org_id}", patch(api::admin::update_org))
        .route("/orgs/{org_id}/members", get(api::admin::org_members))
        .route(
            "/orgs/{org_id}/settings",
            get(api::admin::get_settings).put(api::admin::put_settings),
        )
        .route(
            "/orgs/{org_id}/assignments",
            get(api::admin::list_assignments).post(api::admin::assign),
        )
        .route(
            "/orgs/{org_id}/assignments/{user_id}",
            axum::routing::delete(api::admin::unassign),
        )
        .route("/orgs/{org_id}/audit", get(api::admin::org_audit))
        .route("/ops/agents/exceptions", get(api::agents::staff_exceptions))
        .route(
            "/orgs/{org_id}/agents/auto-send",
            get(api::agents::admin_get_auto_send).put(api::agents::admin_put_auto_send),
        )
        .route("/accounts", get(api::admin::list_accounts))
        .route("/accounts/{user_id}", patch(api::admin::update_account))
        .route(
            "/billing/issuer",
            get(api::billing::get_issuer).put(api::billing::put_issuer),
        )
        .route("/billing/invoices", get(api::billing::list_invoices))
        .route(
            "/shop/settings",
            get(api::shop::get_settings).put(api::shop::put_settings),
        )
        .route("/shop/orders", get(api::shop::admin_orders))
        .route("/shop/orders/{id}", get(api::shop::admin_order))
        .route("/shop/orders/{id}/action", post(api::shop::admin_action))
        .route("/listings", get(api::listings::moderation_list))
        .route("/listings/{id}", get(api::listings::moderation_get))
        .route("/listings/{id}/approve", post(api::listings::approve))
        .route("/listings/{id}/reject", post(api::listings::reject))
        .route("/listings/{id}/retire", post(api::listings::retire))
        .route(
            "/listings/{id}/media/{media_id}/content",
            get(api::listings::moderation_media),
        )
        .route(
            "/orgs/{org_id}/billing/invoices",
            post(api::billing::create_invoice),
        )
        .route(
            "/orgs/{org_id}/billing/invoices/{id}",
            get(api::billing::get_invoice)
                .put(api::billing::update_invoice)
                .delete(api::billing::delete_invoice),
        )
        .route(
            "/orgs/{org_id}/billing/invoices/{id}/issue",
            post(api::billing::issue),
        )
        .route(
            "/orgs/{org_id}/billing/invoices/{id}/payment",
            post(api::billing::record_payment),
        )
        .route(
            "/orgs/{org_id}/billing/invoices/{id}/credit-notes",
            post(api::billing::create_credit_note),
        )
        .route(
            "/orgs/{org_id}/billing/invoices/{id}/pdf",
            get(api::billing::admin_pdf),
        )
        .route(
            "/orgs/{org_id}/savings",
            get(api::savings::admin_get_savings),
        )
        .route(
            "/orgs/{org_id}/savings/agreement",
            axum::routing::put(api::savings::put_agreement),
        )
        .route(
            "/orgs/{org_id}/savings/baselines",
            post(api::savings::add_agreed_baseline),
        )
        .route(
            "/orgs/{org_id}/savings/baselines/from-records",
            post(api::savings::fix_baselines_from_records),
        )
        .route(
            "/orgs/{org_id}/savings/baselines/{baseline_id}",
            axum::routing::delete(api::savings::delete_baseline),
        )
        .route(
            "/orgs/{org_id}/savings/invoice",
            post(api::savings::create_savings_invoice),
        )
        .layer(RequestBodyLimitLayer::new(MAX_JSON_BODY))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(60),
        ));

    Router::new()
        .route(
            "/health",
            get(|| async { Json(serde_json::json!({ "status": "ok" })) }),
        )
        .nest(
            "/api/v1",
            api.merge(uploads)
                .merge(ai)
                .merge(ai_uploads)
                .merge(ai_review),
        )
        .nest("/api/admin/v1", admin)
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
        // Every response: never sniffed as another type, never embedded by other origins,
        // never cached (API data is personal; files are served with their own headers).
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("cross-origin-resource-policy"),
            HeaderValue::from_static("same-origin"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        // HTTPS only from now on when served through a TLS proxy (Tailscale, Caddy); browsers
        // ignore it on plain-HTTP responses, so local development is unaffected.
        .layer(SetResponseHeaderLayer::if_not_present(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000"),
        ))
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
