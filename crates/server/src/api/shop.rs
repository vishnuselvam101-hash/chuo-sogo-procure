//! Consumer shop (Phase E).
//!
//! Individuals sign up (a personal `consumer` organisation each), browse published listings
//! that have a consumer price, and order from Chuo-Sogo. Payment is by bank transfer: when
//! Chuo-Sogo confirms an order, a tax-inclusive qualified invoice with the bank details is
//! issued (Phase B billing); Chuo-Sogo records the payment, ships and completes. Cancelling an
//! invoiced order issues a credit note. Prices, totals and every state change are decided
//! here and in the database, never by the client.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use procure_domain::invoicing::Line;
use procure_domain::matching::TaxRounding;
use procure_domain::money::{Currency, Money};
use procure_domain::rbac::{Access, Permission, Role};
use procure_domain::shop::{self, Actor, OrderStatus};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::api::admin::AdminPrincipal;
use crate::api::auth::{ACTIVATION_TOKEN_TTL, issue_token};
use crate::api::billing::{self, LineInput};
use crate::api::common::{self, Audit};
use crate::api::listings::{self, ListingDetail, ListingSummary, SUMMARY_SELECT};
use crate::api::orgs::{clean_name, normalise_email};
use crate::audit::{self, Event};
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ApiResult, ErrorBody};
use crate::mailer::Mail;
use crate::principal::{ClientIp, Principal};
use crate::ratelimit;

fn today_jst() -> NaiveDate {
    (Utc::now() + Duration::hours(9)).date_naive()
}

// ---------------------------------------------------------------------------------------
// Shop rules and seller notice.

#[derive(Serialize, Deserialize, Clone, sqlx::FromRow, utoipa::ToSchema)]
pub struct ShopSettings {
    /// Orders are accepted only when open (and the notice below is complete).
    pub open: bool,
    /// Shipping per order, tax included.
    pub shipping_fee_minor: i64,
    /// Free shipping from this items total (tax included); empty = never free.
    pub free_shipping_over_minor: Option<i64>,
    /// Days to pay after the order is confirmed.
    pub payment_days: i32,
    pub seller_name: String,
    pub responsible_person: String,
    pub seller_address: String,
    pub seller_phone: String,
    pub seller_email: String,
    pub price_info: String,
    pub payment_info: String,
    pub delivery_info: String,
    pub returns_policy: String,
    pub privacy_policy: String,
}

impl Default for ShopSettings {
    fn default() -> Self {
        ShopSettings {
            open: false,
            shipping_fee_minor: 0,
            free_shipping_over_minor: None,
            payment_days: 7,
            seller_name: String::new(),
            responsible_person: String::new(),
            seller_address: String::new(),
            seller_phone: String::new(),
            seller_email: String::new(),
            price_info: String::new(),
            payment_info: String::new(),
            delivery_info: String::new(),
            returns_policy: String::new(),
            privacy_policy: String::new(),
        }
    }
}

const SETTINGS_COLUMNS: &str = "open, shipping_fee_minor, free_shipping_over_minor, payment_days,
    seller_name, responsible_person, seller_address, seller_phone, seller_email, price_info,
    payment_info, delivery_info, returns_policy, privacy_policy";

async fn load_settings(tx: &mut Tx) -> ApiResult<ShopSettings> {
    Ok(sqlx::query_as::<_, ShopSettings>(&format!(
        // One row, owned by the platform organisation (only it can write it).
        "SELECT {SETTINGS_COLUMNS} FROM shop_settings LIMIT 1"
    ))
    .fetch_optional(&mut **tx)
    .await?
    .unwrap_or_default())
}

#[utoipa::path(
    operation_id = "shop_info",
    get,
    path = "/api/v1/shop/info",
    tag = "shop",
    responses(
        (status = 200, description = "Whether the shop is open, shipping rules and the seller notice (特定商取引法に基づく表記). Public.", body = ShopSettings),
    )
)]
pub async fn info(State(state): State<AppState>) -> ApiResult<Json<ShopSettings>> {
    let mut tx = db::auth_tx(&state.pool).await?;
    db::auth_lookup(&mut tx, true).await?;
    let s = load_settings(&mut tx).await?;
    tx.commit().await?;
    Ok(Json(s))
}

#[utoipa::path(
    operation_id = "admin_put_shop_settings",
    put,
    path = "/api/admin/v1/shop/settings",
    tag = "shop",
    request_body = ShopSettings,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Saved"),
        (status = 422, description = "Invalid, or opening without a complete seller notice", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn put_settings(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiJson(b): ApiJson<ShopSettings>,
) -> ApiResult<StatusCode> {
    let platform = p.require_platform(Permission::ManageShop)?;
    let t = |v: &str, max: usize, field: &'static str| common::text(v, max, true, field);
    let s = ShopSettings {
        open: b.open,
        shipping_fee_minor: b.shipping_fee_minor,
        free_shipping_over_minor: b.free_shipping_over_minor,
        payment_days: b.payment_days,
        seller_name: t(&b.seller_name, 200, "seller_name")?,
        responsible_person: t(&b.responsible_person, 200, "responsible_person")?,
        seller_address: t(&b.seller_address, 500, "seller_address")?,
        seller_phone: t(&b.seller_phone, 50, "seller_phone")?,
        seller_email: t(&b.seller_email, 200, "seller_email")?,
        price_info: t(&b.price_info, 2000, "price_info")?,
        payment_info: t(&b.payment_info, 2000, "payment_info")?,
        delivery_info: t(&b.delivery_info, 2000, "delivery_info")?,
        returns_policy: t(&b.returns_policy, 4000, "returns_policy")?,
        privacy_policy: t(&b.privacy_policy, 8000, "privacy_policy")?,
    };
    if s.shipping_fee_minor < 0 || s.free_shipping_over_minor.is_some_and(|v| v < 0) {
        return Err(ApiError::Validation("shipping_fee_minor"));
    }
    if !(1..=60).contains(&s.payment_days) {
        return Err(ApiError::Validation("payment_days"));
    }
    let notice_complete = [
        &s.seller_name,
        &s.responsible_person,
        &s.seller_address,
        &s.seller_phone,
        &s.seller_email,
        &s.payment_info,
        &s.delivery_info,
        &s.returns_policy,
        &s.privacy_policy,
    ]
    .iter()
    .all(|v| !v.trim().is_empty());
    if s.open && !notice_complete {
        return Err(ApiError::Validation("notice_incomplete"));
    }
    let mut tx = db::tenant_tx(&state.pool, p.user_id, platform).await?;
    db::enable_platform_scope(&mut tx).await?;
    sqlx::query(&format!(
        "INSERT INTO shop_settings (org_id, {SETTINGS_COLUMNS}, updated_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
         ON CONFLICT (org_id) DO UPDATE SET open = EXCLUDED.open,
             shipping_fee_minor = EXCLUDED.shipping_fee_minor,
             free_shipping_over_minor = EXCLUDED.free_shipping_over_minor,
             payment_days = EXCLUDED.payment_days, seller_name = EXCLUDED.seller_name,
             responsible_person = EXCLUDED.responsible_person,
             seller_address = EXCLUDED.seller_address, seller_phone = EXCLUDED.seller_phone,
             seller_email = EXCLUDED.seller_email, price_info = EXCLUDED.price_info,
             payment_info = EXCLUDED.payment_info, delivery_info = EXCLUDED.delivery_info,
             returns_policy = EXCLUDED.returns_policy, privacy_policy = EXCLUDED.privacy_policy,
             updated_by = EXCLUDED.updated_by, updated_at = now()"
    ))
    .bind(platform)
    .bind(s.open)
    .bind(s.shipping_fee_minor)
    .bind(s.free_shipping_over_minor)
    .bind(s.payment_days)
    .bind(&s.seller_name)
    .bind(&s.responsible_person)
    .bind(&s.seller_address)
    .bind(&s.seller_phone)
    .bind(&s.seller_email)
    .bind(&s.price_info)
    .bind(&s.payment_info)
    .bind(&s.delivery_info)
    .bind(&s.returns_policy)
    .bind(&s.privacy_policy)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    Audit {
        org_id: platform,
        p: &p,
    }
    .record(
        &mut tx,
        "shop.settings_updated",
        "organisation",
        platform,
        json!({ "open": s.open, "shipping_fee_minor": s.shipping_fee_minor,
                "free_shipping_over_minor": s.free_shipping_over_minor,
                "payment_days": s.payment_days }),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------------------
// Sign-up.

/// Minimum duration of a sign-up response (longer than the slowest normal path).
const SIGNUP_FLOOR_MS: u64 = 600;

#[derive(Deserialize, utoipa::ToSchema)]
pub struct SignupBody {
    pub email: String,
    pub display_name: String,
    /// `ja` (default), `en`, `ta` or `hi`.
    pub locale: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct SignupResponse {
    /// Always `check_email`: the same answer whether or not the address already has an account.
    pub status: &'static str,
}

#[utoipa::path(
    operation_id = "shop_signup",
    post,
    path = "/api/v1/shop/signup",
    tag = "shop",
    request_body = SignupBody,
    responses(
        (status = 202, description = "If the address is new, an activation email is on its way", body = SignupResponse),
        (status = 422, description = "Invalid email or name", body = ErrorBody),
        (status = 429, description = "Too many sign-ups from this address", body = ErrorBody),
    )
)]
pub async fn signup(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(b): ApiJson<SignupBody>,
) -> ApiResult<(StatusCode, Json<SignupResponse>)> {
    // Switched off (public demo): only from the admin networks, where the demo is seeded.
    if !state.shop_signup && !super::admin::network_allowed(&state, &ip) {
        return Err(ApiError::Forbidden);
    }
    if !state.limiter.check(
        &format!("signup:{}", ratelimit::ip_key(&ip)),
        &ratelimit::SIGNUP_PER_IP,
    ) || !state.limiter.check("signup:all", &ratelimit::SIGNUP_GLOBAL)
    {
        return Err(ApiError::RateLimited);
    }
    // Known and new addresses take about the same time, so timing reveals nothing either.
    let floor = tokio::time::Instant::now() + std::time::Duration::from_millis(SIGNUP_FLOOR_MS);
    let email = normalise_email(&b.email)?;
    let name = clean_name(&b.display_name, 120)?;
    let locale = match b.locale.as_deref().unwrap_or("ja") {
        l @ ("ja" | "en" | "ta" | "hi") => l.to_string(),
        _ => return Err(ApiError::Validation("invalid_locale")),
    };
    let done = Json(SignupResponse {
        status: "check_email",
    });
    let mut tx = db::auth_tx(&state.pool).await?;
    db::auth_lookup(&mut tx, true).await?;
    let existing: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, status FROM users WHERE email = $1")
            .bind(&email)
            .fetch_optional(&mut *tx)
            .await?;
    let existing = match existing {
        Some((user_id, status)) => {
            // Memberships are visible to their own user only.
            sqlx::query("SELECT set_config('app.user_id', $1, true)")
                .bind(user_id.to_string())
                .execute(&mut *tx)
                .await?;
            let consumer: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM memberships WHERE user_id = $1 AND role = 'consumer')",
            )
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?;
            Some((user_id, status, consumer))
        }
        None => None,
    };
    if let Some((user_id, status, consumer)) = existing {
        // Same answer as for a new address: sign-up never reveals who has an account. A
        // consumer account that was never activated gets a fresh code, so nobody can block an
        // address by signing it up first.
        let mail = if status == "pending" && consumer {
            let token = issue_token(&mut tx, user_id, "activation", ACTIVATION_TOKEN_TTL).await?;
            Some(Mail {
                to: email,
                template: "activation",
                locale,
                vars: vec![("token", token)],
            })
        } else {
            None
        };
        tx.commit().await?;
        if let Some(m) = mail {
            state.mailer.send(m).map_err(ApiError::internal)?;
        }
        tokio::time::sleep_until(floor).await;
        return Ok((StatusCode::ACCEPTED, done));
    }
    let user_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO users (id, email, display_name, account_kind, locale)
         VALUES ($1, $2, $3, 'customer', $4)",
    )
    .bind(user_id)
    .bind(&email)
    .bind(&name)
    .bind(&locale)
    .execute(&mut *tx)
    .await?;
    // The personal space is created by the server for this sign-up only.
    db::enable_platform_scope(&mut tx).await?;
    let org_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO organisations (id, kind, name, default_locale) VALUES ($1, 'consumer', $2, $3)",
    )
    .bind(org_id)
    .bind(&name)
    .bind(&locale)
    .execute(&mut *tx)
    .await?;
    db::disable_platform_scope(&mut tx).await?;
    db::set_org(&mut tx, org_id).await?;
    sqlx::query(
        "INSERT INTO memberships (user_id, org_id, role, status) VALUES ($1, $2, $3, 'active')",
    )
    .bind(user_id)
    .bind(org_id)
    .bind(Role::Consumer.code())
    .execute(&mut *tx)
    .await?;
    audit::record(
        &mut tx,
        Event {
            org_id,
            actor: Some(user_id),
            action: "consumer.signed_up",
            entity_type: "user",
            entity_id: Some(user_id.to_string()),
            data: json!({ "ip": ip }),
            request_id: None,
        },
    )
    .await?;
    db::auth_lookup(&mut tx, true).await?;
    let token = issue_token(&mut tx, user_id, "activation", ACTIVATION_TOKEN_TTL).await?;
    tx.commit().await?;
    state
        .mailer
        .send(Mail {
            to: email,
            template: "activation",
            locale,
            vars: vec![("token", token)],
        })
        .map_err(ApiError::internal)?;
    tokio::time::sleep_until(floor).await;
    Ok((StatusCode::ACCEPTED, done))
}

// ---------------------------------------------------------------------------------------
// Consumer side.

/// A consumer acting in their own personal organisation.
async fn consumer_tx(state: &AppState, p: &Principal, org_id: Uuid) -> ApiResult<Tx> {
    let access = p.access(state, org_id).await?;
    if !matches!(access, Access::Member(Role::Consumer)) {
        return Err(ApiError::NotFound);
    }
    common::require_perm(access, Permission::Shop)?;
    Ok(db::tenant_tx(&state.pool, p.user_id, org_id).await?)
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct ShopProductQuery {
    pub q: Option<String>,
}

/// Published, priced for consumers, from an active supplier.
const SHOP_FILTER: &str = "l.status = 'approved' AND l.consumer_price_minor IS NOT NULL
    AND l.currency = 'JPY' AND o.status = 'active'";

#[utoipa::path(
    operation_id = "shop_products",
    get,
    path = "/api/v1/orgs/{org_id}/shop/products",
    tag = "shop",
    params(("org_id" = Uuid, Path), ShopProductQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Products for sale to consumers (prices include tax)", body = [ListingSummary]),
        (status = 404, description = "Not your account", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn products(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiQuery(q): ApiQuery<ShopProductQuery>,
) -> ApiResult<Json<Vec<ListingSummary>>> {
    let search = common::text(q.q.as_deref().unwrap_or(""), 100, false, "q")?;
    let pattern = format!(
        "%{}%",
        search
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let model_key = procure_domain::listing::normalise_model(&search);
    let mut tx = consumer_tx(&state, &p, org_id).await?;
    let rows = sqlx::query_as::<_, ListingSummary>(&format!(
        "{SUMMARY_SELECT} WHERE {SHOP_FILTER}
           AND ($1 = '' OR l.title ILIKE $2 OR l.title_en ILIKE $2 OR l.manufacturer ILIKE $2
                OR ($3 <> '' AND l.model_key LIKE '%' || $3 || '%'))
         ORDER BY l.published_at DESC LIMIT 200"
    ))
    .bind(&search)
    .bind(&pattern)
    .bind(&model_key)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows.into_iter().map(for_consumers).collect()))
}

/// Consumers buy from Chuo-Sogo: they see the consumer price only, never the business price
/// or which supplier lists the product.
fn for_consumers(mut l: ListingSummary) -> ListingSummary {
    l.business_price_minor = None;
    l.supplier_name = String::new();
    l
}

async fn for_sale(tx: &mut Tx, id: Uuid) -> ApiResult<bool> {
    Ok(sqlx::query_scalar(&format!(
        "SELECT EXISTS (SELECT 1 FROM product_listings l JOIN organisations o ON o.id = l.org_id
                        WHERE l.id = $1 AND {SHOP_FILTER})"
    ))
    .bind(id)
    .fetch_one(&mut **tx)
    .await?)
}

#[utoipa::path(
    operation_id = "shop_product",
    get,
    path = "/api/v1/orgs/{org_id}/shop/products/{id}",
    tag = "shop",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Product page", body = ListingDetail),
        (status = 404, description = "Not for sale", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn product(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<ListingDetail>> {
    let mut tx = consumer_tx(&state, &p, org_id).await?;
    if !for_sale(&mut tx, id).await? {
        return Err(ApiError::NotFound);
    }
    let mut d = listings::load_detail(&mut tx, id, false).await?;
    tx.commit().await?;
    d.review_note.clear();
    d.missing.clear();
    d.summary = for_consumers(d.summary);
    d.duplicates.clear();
    Ok(Json(d))
}

#[utoipa::path(
    operation_id = "shop_product_media",
    get,
    path = "/api/v1/orgs/{org_id}/shop/products/{id}/media/{media_id}/content",
    tag = "shop",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path), ("media_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Photo or document of a product for sale"),
        (status = 404, description = "Not for sale", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn product_media(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id, media_id)): ApiPath<(Uuid, Uuid, Uuid)>,
) -> ApiResult<impl IntoResponse> {
    let mut tx = consumer_tx(&state, &p, org_id).await?;
    if !for_sale(&mut tx, id).await? {
        return Err(ApiError::NotFound);
    }
    listings::media_response(&state, tx, id, media_id).await
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ShopOrderLineInput {
    pub listing_id: Uuid,
    pub quantity: u32,
}

#[derive(Deserialize, Serialize, Clone, utoipa::ToSchema)]
pub struct ShippingAddress {
    pub name: String,
    /// 〒 e.g. 100-0001.
    pub postal_code: String,
    /// 都道府県.
    pub prefecture: String,
    pub address_line: String,
    pub phone: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct PlaceOrder {
    pub lines: Vec<ShopOrderLineInput>,
    pub ship_to: ShippingAddress,
    #[serde(default)]
    pub note: String,
    /// The total the consumer saw (tax included); the order is refused if prices changed.
    pub expected_total_minor: i64,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct ShopOrderSummary {
    pub id: Uuid,
    pub org_id: Uuid,
    pub number: String,
    /// `placed`, `confirmed`, `paid`, `shipped`, `completed` or `cancelled`.
    pub status: String,
    pub ship_name: String,
    pub items_total_minor: i64,
    pub shipping_minor: i64,
    pub total_minor: i64,
    pub tax_minor: i64,
    pub placed_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct ShopOrderLine {
    pub position: i32,
    pub listing_id: Uuid,
    pub title: String,
    pub model_number: String,
    pub unit_price_minor: i64,
    pub tax_rate_bps: i32,
    pub quantity: i32,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ShopOrderDetail {
    #[serde(flatten)]
    pub summary: ShopOrderSummary,
    pub ship_to: ShippingAddress,
    pub note: String,
    pub lines: Vec<ShopOrderLine>,
    pub invoice_id: Option<Uuid>,
    pub invoice_number: Option<String>,
    pub invoice_due_date: Option<NaiveDate>,
    pub credit_note_id: Option<Uuid>,
    pub credit_note_number: Option<String>,
    pub carrier: String,
    pub tracking_number: String,
    pub cancel_reason: String,
    pub confirmed_at: Option<DateTime<Utc>>,
    pub paid_at: Option<DateTime<Utc>>,
    pub shipped_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
    /// Admin console only: the consumer's email address.
    pub contact_email: Option<String>,
}

const ORDER_SUMMARY: &str = "SELECT id, org_id, number, status, ship_name, items_total_minor,
    shipping_minor, total_minor, tax_minor, placed_at, updated_at FROM shop_orders";

#[derive(sqlx::FromRow)]
struct ShopOrderRow {
    postal_code: String,
    prefecture: String,
    address_line: String,
    phone: String,
    note: String,
    invoice_id: Option<Uuid>,
    credit_note_id: Option<Uuid>,
    carrier: String,
    tracking_number: String,
    cancel_reason: String,
    confirmed_at: Option<DateTime<Utc>>,
    paid_at: Option<DateTime<Utc>>,
    shipped_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    cancelled_at: Option<DateTime<Utc>>,
}

/// Number and due date of an issued invoice / credit note referenced by an order.
async fn doc_ref(
    tx: &mut Tx,
    doc_id: Option<Uuid>,
) -> ApiResult<(Option<String>, Option<NaiveDate>)> {
    let Some(d) = doc_id else {
        return Ok((None, None));
    };
    Ok(sqlx::query_as::<_, (Option<String>, Option<NaiveDate>)>(
        "SELECT number, due_date FROM issued_invoices WHERE id = $1",
    )
    .bind(d)
    .fetch_optional(&mut **tx)
    .await?
    .unwrap_or((None, None)))
}

async fn load_order(tx: &mut Tx, id: Uuid, with_contact: bool) -> ApiResult<ShopOrderDetail> {
    let summary = sqlx::query_as::<_, ShopOrderSummary>(&format!("{ORDER_SUMMARY} WHERE id = $1"))
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    let r = sqlx::query_as::<_, ShopOrderRow>(
        "SELECT postal_code, prefecture, address_line, phone, note, invoice_id, credit_note_id,
                carrier, tracking_number, cancel_reason, confirmed_at, paid_at, shipped_at,
                completed_at, cancelled_at
         FROM shop_orders WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    let lines = sqlx::query_as::<_, ShopOrderLine>(
        "SELECT position, listing_id, title, model_number, unit_price_minor, tax_rate_bps, quantity
         FROM shop_order_lines WHERE order_id = $1 ORDER BY position",
    )
    .bind(id)
    .fetch_all(&mut **tx)
    .await?;
    let (invoice_number, invoice_due_date) = doc_ref(tx, r.invoice_id).await?;
    let (credit_note_number, _) = doc_ref(tx, r.credit_note_id).await?;
    let contact_email = if with_contact {
        db::auth_lookup(tx, true).await?;
        let email: Option<String> = sqlx::query_scalar(
            "SELECT u.email::text FROM shop_orders s JOIN users u ON u.id = s.placed_by WHERE s.id = $1",
        )
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
        db::auth_lookup(tx, false).await?;
        email
    } else {
        None
    };
    Ok(ShopOrderDetail {
        ship_to: ShippingAddress {
            name: summary.ship_name.clone(),
            postal_code: r.postal_code,
            prefecture: r.prefecture,
            address_line: r.address_line,
            phone: r.phone,
        },
        summary,
        note: r.note,
        lines,
        invoice_id: r.invoice_id,
        invoice_number,
        invoice_due_date,
        credit_note_id: r.credit_note_id,
        credit_note_number,
        carrier: r.carrier,
        tracking_number: r.tracking_number,
        cancel_reason: r.cancel_reason,
        confirmed_at: r.confirmed_at,
        paid_at: r.paid_at,
        shipped_at: r.shipped_at,
        completed_at: r.completed_at,
        cancelled_at: r.cancelled_at,
        contact_email,
    })
}

fn clean_address(a: &ShippingAddress) -> ApiResult<ShippingAddress> {
    let postal = a.postal_code.trim().replace(['〒', ' '], "");
    let digits: String = postal.chars().filter(char::is_ascii_digit).collect();
    if digits.len() != 7 || !postal.chars().all(|c| c.is_ascii_digit() || c == '-') {
        return Err(ApiError::Validation("postal_code"));
    }
    let phone = a.phone.trim().to_string();
    if !(6..=20).contains(&phone.len())
        || !phone
            .chars()
            .all(|c| c.is_ascii_digit() || "+() -".contains(c))
    {
        return Err(ApiError::Validation("phone"));
    }
    let prefecture = common::required(&a.prefecture, 10, false, "prefecture")?;
    if prefecture.chars().count() < 2 {
        return Err(ApiError::Validation("prefecture"));
    }
    Ok(ShippingAddress {
        name: common::required(&a.name, 100, false, "ship_name")?,
        postal_code: format!("{}-{}", &digits[..3], &digits[3..]),
        prefecture,
        address_line: common::required(&a.address_line, 300, false, "address_line")?,
        phone,
    })
}

#[derive(sqlx::FromRow)]
struct ListingForSale {
    supplier: Uuid,
    title: String,
    title_en: String,
    model: String,
    price: Option<i64>,
    rate: i32,
    stock: Option<i32>,
}

/// A short, unguessable order number (orders are not tax documents; invoices carry the
/// gap-free numbers): `SO-YYMMDD-XXXXXX`.
fn order_number() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let r = Uuid::new_v4();
    let code: String = r.as_bytes()[..6]
        .iter()
        .map(|b| char::from(ALPHABET[usize::from(*b) % ALPHABET.len()]))
        .collect();
    format!("SO-{}-{code}", today_jst().format("%y%m%d"))
}

#[utoipa::path(
    operation_id = "shop_place_order",
    post,
    path = "/api/v1/orgs/{org_id}/shop/orders",
    tag = "shop",
    params(("org_id" = Uuid, Path)),
    request_body = PlaceOrder,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Order placed; Chuo-Sogo confirms it and sends the invoice", body = ShopOrderDetail),
        (status = 409, description = "Shop closed, prices changed, or not enough stock", body = ErrorBody),
        (status = 422, description = "Invalid lines or address", body = ErrorBody),
        (status = 404, description = "Not your account, or a product not for sale", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn place_order(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(b): ApiJson<PlaceOrder>,
) -> ApiResult<(StatusCode, Json<ShopOrderDetail>)> {
    if b.lines.is_empty() || b.lines.len() > shop::MAX_ORDER_LINES {
        return Err(ApiError::Validation("lines"));
    }
    let ship = clean_address(&b.ship_to)?;
    let note = common::text(&b.note, 1000, true, "note")?;
    let mut tx = consumer_tx(&state, &p, org_id).await?;
    let settings = load_settings(&mut tx).await?;
    if !settings.open {
        return Err(ApiError::Conflict("shop_closed"));
    }
    struct Priced {
        listing_id: Uuid,
        supplier: Uuid,
        title: String,
        model: String,
        price: i64,
        rate: i32,
        qty: u32,
    }
    let mut priced: Vec<Priced> = Vec::new();
    for l in &b.lines {
        if !(1..=shop::MAX_LINE_QUANTITY).contains(&l.quantity) {
            return Err(ApiError::Validation("quantity"));
        }
        if priced.iter().any(|x| x.listing_id == l.listing_id) {
            return Err(ApiError::Validation("lines"));
        }
        // Current price and stock (the consumer's expected total guards against price changes
        // since they looked; Chuo-Sogo confirms availability before the order is invoiced).
        let row: Option<ListingForSale> = sqlx::query_as(&format!(
            "SELECT l.org_id AS supplier, l.title, l.title_en, l.model_number AS model,
                        l.consumer_price_minor AS price, l.tax_rate_bps AS rate, l.stock
                 FROM product_listings l JOIN organisations o ON o.id = l.org_id
                 WHERE l.id = $1 AND {SHOP_FILTER}"
        ))
        .bind(l.listing_id)
        .fetch_optional(&mut *tx)
        .await?;
        let ListingForSale {
            supplier,
            title,
            title_en,
            model,
            price,
            rate,
            stock,
        } = row.ok_or(ApiError::NotFound)?;
        let price = price.ok_or(ApiError::NotFound)?;
        if stock.is_some_and(|s| i64::from(s) < i64::from(l.quantity)) {
            return Err(ApiError::Conflict("insufficient_stock"));
        }
        priced.push(Priced {
            listing_id: l.listing_id,
            supplier,
            title: if title.trim().is_empty() {
                title_en
            } else {
                title
            },
            model,
            price,
            rate,
            qty: l.quantity,
        });
    }
    let jpy = |m: i64| Money::from_minor(m, Currency::Jpy);
    let domain_lines: Vec<Line> = priced
        .iter()
        .map(|x| Line {
            quantity: x.qty,
            unit_price: jpy(x.price),
            tax_rate_bps: u32::try_from(x.rate).unwrap_or(1000),
        })
        .collect();
    let items = procure_domain::invoicing::totals_inclusive(
        &domain_lines,
        Currency::Jpy,
        TaxRounding::Floor,
    )
    .map_err(|_| ApiError::Validation("amount_overflow"))?;
    let shipping = shop::shipping_fee(
        items.total,
        jpy(settings.shipping_fee_minor),
        settings.free_shipping_over_minor.map(jpy),
    );
    let totals = shop::order_totals(&domain_lines, shipping, Currency::Jpy, TaxRounding::Floor)
        .map_err(|_| ApiError::Validation("amount_overflow"))?;
    if totals.total.minor() != b.expected_total_minor {
        return Err(ApiError::Conflict("price_changed"));
    }
    let id = Uuid::now_v7();
    let number = order_number();
    sqlx::query(
        "INSERT INTO shop_orders (id, org_id, number, ship_name, postal_code, prefecture,
             address_line, phone, note, items_total_minor, shipping_minor, total_minor, tax_minor,
             placed_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
    )
    .bind(id)
    .bind(org_id)
    .bind(&number)
    .bind(&ship.name)
    .bind(&ship.postal_code)
    .bind(&ship.prefecture)
    .bind(&ship.address_line)
    .bind(&ship.phone)
    .bind(&note)
    .bind(items.total.minor())
    .bind(shipping.minor())
    .bind(totals.total.minor())
    .bind(totals.tax.minor())
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    for (i, x) in priced.iter().enumerate() {
        sqlx::query(
            "INSERT INTO shop_order_lines (id, org_id, order_id, position, listing_id,
                 supplier_org_id, title, model_number, unit_price_minor, tax_rate_bps, quantity)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(Uuid::now_v7())
        .bind(org_id)
        .bind(id)
        .bind(i32::try_from(i + 1).map_err(ApiError::internal)?)
        .bind(x.listing_id)
        .bind(x.supplier)
        .bind(&x.title)
        .bind(&x.model)
        .bind(x.price)
        .bind(x.rate)
        .bind(i32::try_from(x.qty).map_err(ApiError::internal)?)
        .execute(&mut *tx)
        .await?;
    }
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "shop.order_placed",
            "shop_order",
            id,
            json!({ "number": number, "total_minor": totals.total.minor(), "lines": priced.len() }),
        )
        .await?;
    let detail = load_order(&mut tx, id, false).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(detail)))
}

#[utoipa::path(
    operation_id = "shop_orders",
    get,
    path = "/api/v1/orgs/{org_id}/shop/orders",
    tag = "shop",
    params(("org_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Your orders, newest first", body = [ShopOrderSummary]),
        (status = 404, description = "Not your account", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn my_orders(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
) -> ApiResult<Json<Vec<ShopOrderSummary>>> {
    let mut tx = consumer_tx(&state, &p, org_id).await?;
    let rows = sqlx::query_as::<_, ShopOrderSummary>(&format!(
        "{ORDER_SUMMARY} WHERE org_id = $1 ORDER BY placed_at DESC LIMIT 200"
    ))
    .bind(org_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[utoipa::path(
    operation_id = "shop_order",
    get,
    path = "/api/v1/orgs/{org_id}/shop/orders/{id}",
    tag = "shop",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Order with its lines, invoice and shipping", body = ShopOrderDetail),
        (status = 404, description = "Not found", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn my_order(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<ShopOrderDetail>> {
    let mut tx = consumer_tx(&state, &p, org_id).await?;
    let d = load_order(&mut tx, id, false).await?;
    if d.summary.org_id != org_id {
        return Err(ApiError::NotFound);
    }
    tx.commit().await?;
    Ok(Json(d))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ShopCancelBody {
    #[serde(default)]
    pub reason: String,
}

#[utoipa::path(
    operation_id = "shop_cancel_order",
    post,
    path = "/api/v1/orgs/{org_id}/shop/orders/{id}/cancel",
    tag = "shop",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = ShopCancelBody,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Cancelled"),
        (status = 409, description = "Already confirmed (contact Chuo-Sogo)", body = ErrorBody),
        (status = 404, description = "Not found", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn cancel_mine(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(b): ApiJson<ShopCancelBody>,
) -> ApiResult<StatusCode> {
    let reason = common::text(&b.reason, 1000, true, "reason")?;
    let mut tx = consumer_tx(&state, &p, org_id).await?;
    let from = lock_order(&mut tx, id, Some(org_id)).await?;
    if !shop::can_move(from, OrderStatus::Cancelled, Actor::Consumer) {
        return Err(ApiError::Conflict("invalid_transition"));
    }
    sqlx::query(
        "UPDATE shop_orders SET status = 'cancelled', cancelled_at = now(), cancel_reason = $2,
             updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(&reason)
    .execute(&mut *tx)
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "shop.order_cancelled",
            "shop_order",
            id,
            json!({ "by": "consumer" }),
        )
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn lock_order(tx: &mut Tx, id: Uuid, org_id: Option<Uuid>) -> ApiResult<OrderStatus> {
    let row: Option<(String, Uuid)> =
        sqlx::query_as("SELECT status, org_id FROM shop_orders WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
    let (status, owner) = row.ok_or(ApiError::NotFound)?;
    if org_id.is_some_and(|o| o != owner) {
        return Err(ApiError::NotFound);
    }
    OrderStatus::from_code(&status)
        .ok_or_else(|| ApiError::internal(std::io::Error::other("bad order status")))
}

// ---------------------------------------------------------------------------------------
// Shop staff (admin console).

async fn staff_tx(state: &AppState, p: &Principal) -> ApiResult<(Tx, Uuid)> {
    let platform = p.require_platform(Permission::ManageShop)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, platform).await?;
    db::enable_platform_scope(&mut tx).await?;
    Ok((tx, platform))
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct ShopOrderQuery {
    pub status: Option<String>,
}

#[utoipa::path(
    operation_id = "admin_shop_orders",
    get,
    path = "/api/admin/v1/shop/orders",
    tag = "shop",
    params(ShopOrderQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Orders (oldest open first)", body = [ShopOrderSummary]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn admin_orders(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiQuery(q): ApiQuery<ShopOrderQuery>,
) -> ApiResult<Json<Vec<ShopOrderSummary>>> {
    if q.status
        .as_deref()
        .is_some_and(|s| OrderStatus::from_code(s).is_none())
    {
        return Err(ApiError::Validation("status"));
    }
    let (mut tx, _) = staff_tx(&state, &p).await?;
    let rows = sqlx::query_as::<_, ShopOrderSummary>(&format!(
        "{ORDER_SUMMARY} WHERE ($1::text IS NULL OR status = $1) ORDER BY placed_at LIMIT 500"
    ))
    .bind(&q.status)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[utoipa::path(
    operation_id = "admin_shop_order",
    get,
    path = "/api/admin/v1/shop/orders/{id}",
    tag = "shop",
    params(("id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Order with contact email", body = ShopOrderDetail),
        (status = 404, description = "Not found", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn admin_order(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<Json<ShopOrderDetail>> {
    let (mut tx, _) = staff_tx(&state, &p).await?;
    let d = load_order(&mut tx, id, true).await?;
    tx.commit().await?;
    Ok(Json(d))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ShopOrderAction {
    /// `confirm`, `paid`, `ship`, `complete` or `cancel`.
    pub action: String,
    /// `paid`: the day the transfer arrived.
    pub paid_on: Option<NaiveDate>,
    #[serde(default)]
    pub carrier: String,
    #[serde(default)]
    pub tracking_number: String,
    /// `cancel`: required.
    #[serde(default)]
    pub reason: String,
}

#[utoipa::path(
    operation_id = "admin_shop_order_action",
    post,
    path = "/api/admin/v1/shop/orders/{id}/action",
    tag = "shop",
    params(("id" = Uuid, Path)),
    request_body = ShopOrderAction,
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Done; the order as it is now", body = ShopOrderDetail),
        (status = 409, description = "Not possible in this state, or billing not set up", body = ErrorBody),
        (status = 422, description = "Missing date, tracking number or reason", body = ErrorBody),
        (status = 404, description = "Not found", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn admin_action(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(b): ApiJson<ShopOrderAction>,
) -> ApiResult<Json<ShopOrderDetail>> {
    let to = match b.action.as_str() {
        "confirm" => OrderStatus::Confirmed,
        "paid" => OrderStatus::Paid,
        "ship" => OrderStatus::Shipped,
        "complete" => OrderStatus::Completed,
        "cancel" => OrderStatus::Cancelled,
        _ => return Err(ApiError::Validation("action")),
    };
    let (mut tx, platform) = staff_tx(&state, &p).await?;
    // The billing issuer profile and shop rules are read while app.org_id is the platform.
    let settings = load_settings(&mut tx).await?;
    let issuer: Option<billing::IssuerProfile> = sqlx::query_as(
        "SELECT legal_name, registration_number, address, phone, email, bank_details,
                payment_terms_days, tax_rounding, number_prefix
         FROM issuer_profile WHERE org_id = $1",
    )
    .bind(platform)
    .fetch_optional(&mut *tx)
    .await?;
    let org_id: Uuid = sqlx::query_scalar("SELECT org_id FROM shop_orders WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    db::set_org(&mut tx, org_id).await?;
    let from = lock_order(&mut tx, id, None).await?;
    if !shop::can_move(from, to, Actor::Shop) {
        return Err(ApiError::Conflict("invalid_transition"));
    }
    let order = load_order(&mut tx, id, true).await?;
    // The customer's own language for notifications.
    let locale: String = sqlx::query_scalar(
        "SELECT coalesce(u.locale, 'ja') FROM shop_orders s JOIN users u ON u.id = s.placed_by
         WHERE s.id = $1",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    let mut mail: Option<Mail> = None;
    let mut data = json!({ "from": from.code(), "to": to.code(), "number": order.summary.number });
    match to {
        OrderStatus::Confirmed => {
            let issuer = issuer.ok_or(ApiError::Conflict("issuer_not_configured"))?;
            // Reserve stock now (re-checked under a lock): confirmed orders never oversell.
            let mut reserved = Vec::new();
            for l in &order.lines {
                let stock: Option<i32> = sqlx::query_scalar(
                    "SELECT stock FROM product_listings WHERE id = $1 FOR UPDATE",
                )
                .bind(l.listing_id)
                .fetch_optional(&mut *tx)
                .await?
                .flatten();
                if let Some(s) = stock {
                    if s < l.quantity {
                        return Err(ApiError::Conflict("insufficient_stock"));
                    }
                    sqlx::query(
                        "UPDATE product_listings SET stock = stock - $2, updated_at = now()
                         WHERE id = $1",
                    )
                    .bind(l.listing_id)
                    .bind(l.quantity)
                    .execute(&mut *tx)
                    .await?;
                    reserved.push(json!({ "listing_id": l.listing_id, "quantity": l.quantity }));
                }
            }
            let due = today_jst() + Duration::days(i64::from(settings.payment_days));
            let mut lines: Vec<LineInput> = order
                .lines
                .iter()
                .map(|l| LineInput {
                    description: format!("{} ({})", l.title, l.model_number)
                        .chars()
                        .take(500)
                        .collect(),
                    quantity: u32::try_from(l.quantity).unwrap_or(1),
                    unit: String::new(),
                    unit_price_minor: l.unit_price_minor,
                    tax_rate_bps: u32::try_from(l.tax_rate_bps).unwrap_or(1000),
                })
                .collect();
            if order.summary.shipping_minor > 0 {
                lines.push(LineInput {
                    description: "送料 Shipping".into(),
                    quantity: 1,
                    unit: String::new(),
                    unit_price_minor: order.summary.shipping_minor,
                    tax_rate_bps: 1000,
                });
            }
            let recipient_address = format!(
                "〒{} {}{}",
                order.ship_to.postal_code, order.ship_to.prefecture, order.ship_to.address_line
            );
            let notes = format!(
                "ご注文番号 Order {}\n{}",
                order.summary.number, settings.payment_info
            );
            let invoice_id = billing::shop_invoice_in_tx(
                &mut tx,
                &p,
                platform,
                org_id,
                &order.ship_to.name,
                &recipient_address,
                due,
                &notes.chars().take(2000).collect::<String>(),
                &lines,
                &issuer.tax_rounding,
            )
            .await?;
            db::set_org(&mut tx, org_id).await?;
            let invoice = billing::issue_in_tx(
                &state, &mut tx, &p, platform, org_id, &issuer, invoice_id, None,
            )
            .await?;
            if invoice.summary.total_minor != order.summary.total_minor {
                // Inclusive totals cannot differ; refuse rather than bill a different amount.
                return Err(ApiError::internal(std::io::Error::other(
                    "invoice total differs from the order total",
                )));
            }
            db::set_org(&mut tx, org_id).await?;
            sqlx::query(
                "UPDATE shop_orders SET status = 'confirmed', invoice_id = $2, reserved = $3,
                     confirmed_at = now(), updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(invoice_id)
            .bind(serde_json::Value::Array(reserved))
            .execute(&mut *tx)
            .await?;
            data["invoice"] = json!(invoice.summary.number);
            mail = Some(Mail {
                to: order.contact_email.clone().unwrap_or_default(),
                template: "shop_order_confirmed",
                locale: locale.clone(),
                vars: vec![
                    ("order", order.summary.number.clone()),
                    (
                        "invoice",
                        invoice.summary.number.clone().unwrap_or_default(),
                    ),
                    ("total", order.summary.total_minor.to_string()),
                    ("due", due.to_string()),
                    ("bank", issuer.bank_details.clone()),
                ],
            });
        }
        OrderStatus::Paid => {
            let paid_on = b.paid_on.ok_or(ApiError::Validation("paid_on"))?;
            if paid_on > today_jst() {
                return Err(ApiError::Validation("paid_on"));
            }
            let invoice_id = order.invoice_id.ok_or(ApiError::Conflict("not_issued"))?;
            sqlx::query(
                "UPDATE issued_invoices SET status = 'paid', paid_on = $2, paid_by = $3,
                     paid_note = $4, updated_at = now()
                 WHERE id = $1 AND status = 'issued'",
            )
            .bind(invoice_id)
            .bind(paid_on)
            .bind(p.user_id)
            .bind(format!("shop order {}", order.summary.number))
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE shop_orders SET status = 'paid', paid_at = now(), updated_at = now()
                 WHERE id = $1",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
            data["paid_on"] = json!(paid_on);
        }
        OrderStatus::Shipped => {
            let carrier = common::required(&b.carrier, 100, false, "carrier")?;
            let tracking = common::required(&b.tracking_number, 100, false, "tracking_number")?;
            sqlx::query(
                "UPDATE shop_orders SET status = 'shipped', carrier = $2, tracking_number = $3,
                     shipped_at = now(), updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(&carrier)
            .bind(&tracking)
            .execute(&mut *tx)
            .await?;
            data["tracking"] = json!(format!("{carrier} {tracking}"));
            mail = Some(Mail {
                to: order.contact_email.clone().unwrap_or_default(),
                template: "shop_order_shipped",
                locale: locale.clone(),
                vars: vec![
                    ("order", order.summary.number.clone()),
                    ("carrier", carrier),
                    ("tracking", tracking),
                ],
            });
        }
        OrderStatus::Completed => {
            sqlx::query(
                "UPDATE shop_orders SET status = 'completed', completed_at = now(),
                     updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
        OrderStatus::Cancelled => {
            let reason = common::required(&b.reason, 1000, true, "reason")?;
            // Return reserved stock (only to listings that still track stock).
            let reserved: serde_json::Value =
                sqlx::query_scalar("SELECT reserved FROM shop_orders WHERE id = $1")
                    .bind(id)
                    .fetch_one(&mut *tx)
                    .await?;
            for r in reserved.as_array().into_iter().flatten() {
                let (Some(listing), Some(qty)) = (
                    r["listing_id"]
                        .as_str()
                        .and_then(|s| s.parse::<Uuid>().ok()),
                    r["quantity"].as_i64(),
                ) else {
                    continue;
                };
                sqlx::query(
                    "UPDATE product_listings SET stock = stock + $2, updated_at = now()
                     WHERE id = $1 AND stock IS NOT NULL",
                )
                .bind(listing)
                .bind(i32::try_from(qty).map_err(ApiError::internal)?)
                .execute(&mut *tx)
                .await?;
            }
            db::set_org(&mut tx, org_id).await?;
            let mut credit_note_id = None;
            if let Some(invoice_id) = order.invoice_id {
                // The invoice is a tax document: it is cancelled by a credit note for all of it.
                let lines = sqlx::query_as::<_, (String, i32, String, i64, i32)>(
                    "SELECT description, quantity, unit, unit_price_minor, tax_rate_bps
                     FROM issued_invoice_lines WHERE invoice_id = $1 ORDER BY position",
                )
                .bind(invoice_id)
                .fetch_all(&mut *tx)
                .await?
                .into_iter()
                .map(|(description, q, unit, price, rate)| LineInput {
                    description,
                    quantity: u32::try_from(q).unwrap_or(1),
                    unit,
                    unit_price_minor: price,
                    tax_rate_bps: u32::try_from(rate).unwrap_or(1000),
                })
                .collect::<Vec<_>>();
                let issuer = issuer.ok_or(ApiError::Conflict("issuer_not_configured"))?;
                let cn = billing::credit_in_tx(
                    &mut tx,
                    &p,
                    platform,
                    org_id,
                    invoice_id,
                    &format!(
                        "ご注文 {} の取消 Order cancelled: {reason}",
                        order.summary.number
                    )
                    .chars()
                    .take(2000)
                    .collect::<String>(),
                    "",
                    &lines,
                )
                .await?;
                db::set_org(&mut tx, org_id).await?;
                billing::issue_in_tx(&state, &mut tx, &p, platform, org_id, &issuer, cn, None)
                    .await?;
                credit_note_id = Some(cn);
            }
            db::set_org(&mut tx, org_id).await?;
            sqlx::query(
                "UPDATE shop_orders SET status = 'cancelled', cancel_reason = $2,
                     credit_note_id = $3, cancelled_at = now(), updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(&reason)
            .bind(credit_note_id)
            .execute(&mut *tx)
            .await?;
            data["reason"] = json!(reason);
        }
        OrderStatus::Placed => return Err(ApiError::Validation("action")),
    }
    for target in [platform, org_id] {
        Audit {
            org_id: target,
            p: &p,
        }
        .record(
            &mut tx,
            "shop.order_updated",
            "shop_order",
            id,
            data.clone(),
        )
        .await?;
    }
    db::set_org(&mut tx, org_id).await?;
    let detail = load_order(&mut tx, id, true).await?;
    tx.commit().await?;
    if let Some(m) = mail.filter(|m| !m.to.is_empty()) {
        // After commit: a mail problem never undoes the order change.
        if let Err(e) = state.mailer.send(m) {
            tracing::warn!(error = %e, "shop notification not sent");
        }
    }
    Ok(Json(detail))
}

#[utoipa::path(
    operation_id = "admin_get_shop_settings",
    get,
    path = "/api/admin/v1/shop/settings",
    tag = "shop",
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Shop rules and seller notice", body = ShopSettings),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn get_settings(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
) -> ApiResult<Json<ShopSettings>> {
    let (mut tx, _) = staff_tx(&state, &p).await?;
    let s = load_settings(&mut tx).await?;
    tx.commit().await?;
    Ok(Json(s))
}

// ---------------------------------------------------------------------------------------
// Privacy: a consumer's own data, and closing the account (個人情報保護法).

#[derive(Serialize, utoipa::ToSchema)]
pub struct MyData {
    pub email: String,
    pub display_name: String,
    pub locale: Option<String>,
    pub created_at: DateTime<Utc>,
    pub orders: Vec<ShopOrderDetail>,
    /// What happens to this data when the account is closed.
    pub retention: &'static str,
}

const RETENTION: &str = "Orders and invoices are kept for the period required by Japanese tax \
law (7 years) and then deleted; everything else is removed when the account is closed.";

#[utoipa::path(
    operation_id = "shop_my_data",
    get,
    path = "/api/v1/orgs/{org_id}/shop/my-data",
    tag = "shop",
    params(("org_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Everything the shop holds about you", body = MyData),
        (status = 404, description = "Not your account", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn my_data(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
) -> ApiResult<Json<MyData>> {
    let mut tx = consumer_tx(&state, &p, org_id).await?;
    let (email, display_name, locale, created_at): (String, String, Option<String>, DateTime<Utc>) =
        sqlx::query_as(
            "SELECT email::text, display_name, locale, created_at FROM users WHERE id = $1",
        )
        .bind(p.user_id)
        .fetch_one(&mut *tx)
        .await?;
    let ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM shop_orders WHERE org_id = $1 ORDER BY placed_at")
            .bind(org_id)
            .fetch_all(&mut *tx)
            .await?;
    let mut orders = Vec::with_capacity(ids.len());
    for id in ids {
        orders.push(load_order(&mut tx, id, false).await?);
    }
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "consumer.data_exported",
            "user",
            p.user_id,
            json!({}),
        )
        .await?;
    tx.commit().await?;
    Ok(Json(MyData {
        email,
        display_name,
        locale,
        created_at,
        orders,
        retention: RETENTION,
    }))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct CloseAccount {
    /// Must be `DELETE` (a deliberate action).
    pub confirm: String,
}

#[utoipa::path(
    operation_id = "shop_close_account",
    post,
    path = "/api/v1/orgs/{org_id}/shop/close-account",
    tag = "shop",
    params(("org_id" = Uuid, Path)),
    request_body = CloseAccount,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Account closed: personal details removed, signed out everywhere"),
        (status = 409, description = "An order is still open (wait until it is completed or cancelled)", body = ErrorBody),
        (status = 422, description = "Confirmation missing", body = ErrorBody),
        (status = 404, description = "Not your account", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn close_account(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(b): ApiJson<CloseAccount>,
) -> ApiResult<StatusCode> {
    if b.confirm != "DELETE" {
        return Err(ApiError::Validation("confirm"));
    }
    let mut tx = consumer_tx(&state, &p, org_id).await?;
    let open: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM shop_orders WHERE org_id = $1
                        AND status IN ('placed', 'confirmed', 'paid', 'shipped'))",
    )
    .bind(org_id)
    .fetch_one(&mut *tx)
    .await?;
    if open {
        return Err(ApiError::Conflict("open_orders"));
    }
    // The login identity and name go; orders and invoices stay for the legal retention period
    // (they carry the delivery name and address the invoice was issued to).
    let placeholder = format!("closed-{}@invalid.example", Uuid::new_v4());
    sqlx::query(
        "UPDATE users SET email = $2, display_name = '(closed account)', locale = NULL,
             password_hash = NULL, status = 'deactivated', totp_secret_enc = NULL,
             totp_enabled_at = NULL, totp_last_step = NULL
         WHERE id = $1",
    )
    .bind(p.user_id)
    .bind(&placeholder)
    .execute(&mut *tx)
    .await?;
    // The personal space was named after the person; only the server renames it.
    db::enable_platform_scope(&mut tx).await?;
    sqlx::query(
        "UPDATE organisations SET name = '(closed account)' WHERE id = $1 AND kind = 'consumer'",
    )
    .bind(org_id)
    .execute(&mut *tx)
    .await?;
    db::disable_platform_scope(&mut tx).await?;
    sqlx::query("UPDATE sessions SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL")
        .bind(p.user_id)
        .execute(&mut *tx)
        .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "consumer.account_closed",
            "user",
            p.user_id,
            json!({}),
        )
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
