//! Invoices Chuo-Sogo issues to its customers, and credit notes (Phase B).
//!
//! Admin console (`/api/admin/v1`, permission `manage_billing`): issuer profile, drafts,
//! issuing (gap-free number, PDF, immutable from then on), payment status, credit notes.
//! Business app (`/api/v1`, permission `view_finance`): a customer's issued documents and PDFs.
//!
//! Amounts are computed only here, from the lines (`procure_domain::invoicing`); the database
//! refuses any change to an issued document except its payment status.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::IntoResponse;
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use procure_domain::invoicing::{self, InvoiceError, Line, RateTotal};
use procure_domain::matching::TaxRounding;
use procure_domain::money::{Currency, Money};
use procure_domain::rbac::{Permission, allows as rbac_allows};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::api::admin::AdminPrincipal;
use crate::api::common::{self, Audit};
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ApiResult, ErrorBody};
use crate::invoice_pdf;
use crate::principal::Principal;

/// Invoices are dated in Japan time.
fn today_jst() -> NaiveDate {
    (Utc::now() + Duration::hours(9)).date_naive()
}

fn invoice_error(e: InvoiceError) -> ApiError {
    match e {
        InvoiceError::NoLines => ApiError::Validation("lines"),
        InvoiceError::TooManyLines => ApiError::Validation("too_many_lines"),
        InvoiceError::Quantity => ApiError::Validation("quantity"),
        InvoiceError::RateNotAllowed(_) => ApiError::Validation("tax_rate_bps"),
        InvoiceError::CurrencyMismatch => ApiError::Validation("currency"),
        InvoiceError::Money(_) => ApiError::Validation("amount_overflow"),
        InvoiceError::ExceedsOriginal { .. } => ApiError::Conflict("credit_exceeds_original"),
    }
}

fn rounding(code: &str) -> ApiResult<TaxRounding> {
    TaxRounding::from_code(code).ok_or(ApiError::Validation("tax_rounding"))
}

// ---------------------------------------------------------------------------------------
// Issuer profile.

#[derive(Serialize, Deserialize, Clone, sqlx::FromRow, utoipa::ToSchema)]
pub struct IssuerProfile {
    pub legal_name: String,
    /// `T` + 13 digits (qualified invoice issuer registration number).
    pub registration_number: String,
    pub address: String,
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub email: String,
    /// Bank transfer details printed on invoices.
    #[serde(default)]
    pub bank_details: String,
    pub payment_terms_days: i32,
    /// `floor`, `half_up` or `ceil` (once per rate per invoice).
    pub tax_rounding: String,
    /// Invoice number prefix (A–Z, 0–9, up to 8).
    pub number_prefix: String,
}

/// Opens a platform-scoped transaction after checking `manage_billing`; returns it with the
/// platform organisation's id (app.org_id is set to it).
async fn billing_tx(state: &AppState, p: &Principal) -> ApiResult<(Tx, Uuid)> {
    let platform = p.require_platform(Permission::ManageBilling)?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, platform).await?;
    db::enable_platform_scope(&mut tx).await?;
    Ok((tx, platform))
}

async fn load_issuer(tx: &mut Tx, platform: Uuid) -> ApiResult<Option<IssuerProfile>> {
    Ok(sqlx::query_as::<_, IssuerProfile>(
        "SELECT legal_name, registration_number, address, phone, email, bank_details,
                payment_terms_days, tax_rounding, number_prefix
         FROM issuer_profile WHERE org_id = $1",
    )
    .bind(platform)
    .fetch_optional(&mut **tx)
    .await?)
}

#[utoipa::path(
    operation_id = "admin_get_issuer",
    get,
    path = "/api/admin/v1/billing/issuer",
    tag = "billing",
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Issuer details printed on invoices", body = IssuerProfile),
        (status = 404, description = "Not set up yet", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn get_issuer(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
) -> ApiResult<Json<IssuerProfile>> {
    let (mut tx, platform) = billing_tx(&state, &p).await?;
    let profile = load_issuer(&mut tx, platform).await?;
    tx.commit().await?;
    profile.map(Json).ok_or(ApiError::NotFound)
}

#[utoipa::path(
    operation_id = "admin_put_issuer",
    put,
    path = "/api/admin/v1/billing/issuer",
    tag = "billing",
    request_body = IssuerProfile,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Saved (applies to documents issued from now on)"),
        (status = 422, description = "Invalid input (e.g. registration number check digit)", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn put_issuer(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiJson(body): ApiJson<IssuerProfile>,
) -> ApiResult<StatusCode> {
    let legal_name = common::required(&body.legal_name, 200, false, "legal_name")?;
    let registration_number = body.registration_number.trim().to_ascii_uppercase();
    if !invoicing::registration_number_valid(&registration_number) {
        return Err(ApiError::Validation("registration_number"));
    }
    let address = common::required(&body.address, 500, true, "address")?;
    let phone = common::text(&body.phone, 50, false, "phone")?;
    let email = common::text(&body.email, 200, false, "email")?;
    let bank_details = common::text(&body.bank_details, 1000, true, "bank_details")?;
    if !(0..=365).contains(&body.payment_terms_days) {
        return Err(ApiError::Validation("payment_terms_days"));
    }
    rounding(&body.tax_rounding)?;
    let prefix = body.number_prefix.trim().to_string();
    if prefix.is_empty()
        || prefix.len() > 8
        || !prefix
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return Err(ApiError::Validation("number_prefix"));
    }
    let (mut tx, platform) = billing_tx(&state, &p).await?;
    sqlx::query(
        "INSERT INTO issuer_profile (org_id, legal_name, registration_number, address, phone, email,
                                     bank_details, payment_terms_days, tax_rounding, number_prefix,
                                     updated_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
         ON CONFLICT (org_id) DO UPDATE SET legal_name = EXCLUDED.legal_name,
             registration_number = EXCLUDED.registration_number, address = EXCLUDED.address,
             phone = EXCLUDED.phone, email = EXCLUDED.email, bank_details = EXCLUDED.bank_details,
             payment_terms_days = EXCLUDED.payment_terms_days, tax_rounding = EXCLUDED.tax_rounding,
             number_prefix = EXCLUDED.number_prefix, updated_by = EXCLUDED.updated_by,
             updated_at = now()",
    )
    .bind(platform)
    .bind(&legal_name)
    .bind(&registration_number)
    .bind(&address)
    .bind(&phone)
    .bind(&email)
    .bind(&bank_details)
    .bind(body.payment_terms_days)
    .bind(&body.tax_rounding)
    .bind(&prefix)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    Audit {
        org_id: platform,
        p: &p,
    }
    .record(
        &mut tx,
        "billing.issuer_updated",
        "organisation",
        platform,
        json!({ "legal_name": legal_name, "registration_number": registration_number,
                "payment_terms_days": body.payment_terms_days, "tax_rounding": body.tax_rounding,
                "number_prefix": prefix }),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------------------
// Documents.

#[derive(Deserialize, Serialize, Clone, utoipa::ToSchema)]
pub struct LineInput {
    pub description: String,
    pub quantity: u32,
    #[serde(default)]
    pub unit: String,
    /// Unit price in minor units, tax excluded.
    pub unit_price_minor: i64,
    /// 1000 (10 %), 800 (8 % reduced rate) or 0 (exempt).
    pub tax_rate_bps: u32,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct InvoiceInput {
    /// ISO 4217; default JPY.
    pub currency: Option<String>,
    /// Default: the organisation's name.
    pub recipient_name: Option<String>,
    #[serde(default)]
    pub recipient_address: String,
    /// Default at issue: issue date + the issuer's payment terms.
    pub due_date: Option<NaiveDate>,
    /// Optional procurement request this fee relates to.
    pub request_id: Option<Uuid>,
    #[serde(default)]
    pub notes: String,
    pub lines: Vec<LineInput>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreditNoteInput {
    /// Why the amount is returned (printed on the credit note).
    pub reason: String,
    #[serde(default)]
    pub notes: String,
    pub lines: Vec<LineInput>,
}

#[derive(Serialize, Deserialize, Clone, utoipa::ToSchema)]
pub struct RateTotalView {
    pub rate_bps: u32,
    pub base_minor: i64,
    pub tax_minor: i64,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct InvoiceSummary {
    pub id: Uuid,
    pub org_id: Uuid,
    pub org_name: String,
    /// `invoice` or `credit_note`.
    pub kind: String,
    /// `draft`, `issued` or `paid`.
    pub status: String,
    pub number: Option<String>,
    pub original_id: Option<Uuid>,
    pub issue_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub currency: String,
    pub total_minor: i64,
    /// Issued, unpaid and past its due date (Japan time).
    pub overdue: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct LineView {
    pub position: i32,
    pub description: String,
    pub quantity: i32,
    pub unit: String,
    pub unit_price_minor: i64,
    pub tax_rate_bps: i32,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct InvoiceDetail {
    #[serde(flatten)]
    pub summary: InvoiceSummary,
    pub recipient_name: String,
    pub recipient_address: String,
    pub request_id: Option<Uuid>,
    pub notes: String,
    pub reason: String,
    pub tax_rounding: String,
    pub subtotal_minor: i64,
    pub tax_minor: i64,
    pub per_rate: Vec<RateTotalView>,
    pub lines: Vec<LineView>,
    /// Credit notes issued against this invoice (any status, admin only; issued for customers).
    pub credit_notes: Vec<InvoiceSummary>,
    pub paid_on: Option<NaiveDate>,
    pub paid_note: String,
    /// SHA-256 of the issued PDF (hex), also recorded in the audit log.
    pub pdf_sha256: Option<String>,
}

const SUMMARY_SELECT: &str = "SELECT i.id, i.org_id, o.name AS org_name, i.kind, i.status, i.number,
        i.original_id, i.issue_date, i.due_date, i.currency, i.total_minor,
        (i.status = 'issued' AND i.kind = 'invoice' AND i.due_date < (now() AT TIME ZONE 'Asia/Tokyo')::date) AS overdue,
        i.created_at
    FROM issued_invoices i JOIN organisations o ON o.id = i.org_id";

fn domain_lines(lines: &[LineInput], currency: Currency) -> ApiResult<Vec<Line>> {
    lines
        .iter()
        .map(|l| {
            if l.unit_price_minor < 0 {
                return Err(ApiError::Validation("unit_price_minor"));
            }
            Ok(Line {
                quantity: l.quantity,
                unit_price: Money::from_minor(l.unit_price_minor, currency),
                tax_rate_bps: l.tax_rate_bps,
            })
        })
        .collect()
}

struct Computed {
    subtotal: i64,
    tax: i64,
    total: i64,
    per_rate: Vec<RateTotalView>,
}

/// `inclusive`: unit prices include tax (consumer sales); the tax contained is computed per
/// rate. Otherwise prices exclude tax and the tax is added per rate.
fn compute(
    lines: &[LineInput],
    currency: Currency,
    r: TaxRounding,
    inclusive: bool,
) -> ApiResult<Computed> {
    for l in lines {
        common::required(&l.description, 500, false, "description")?;
        common::text(&l.unit, 20, false, "unit")?;
    }
    let domain = domain_lines(lines, currency)?;
    let t = if inclusive {
        invoicing::totals_inclusive(&domain, currency, r)
    } else {
        invoicing::totals(&domain, currency, r)
    }
    .map_err(invoice_error)?;
    Ok(Computed {
        subtotal: t.subtotal.minor(),
        tax: t.tax.minor(),
        total: t.total.minor(),
        per_rate: t
            .per_rate
            .iter()
            .map(|r| RateTotalView {
                rate_bps: r.rate_bps,
                base_minor: r.base.minor(),
                tax_minor: r.tax.minor(),
            })
            .collect(),
    })
}

/// Totals of a credit note after `limit_credit` capped its tax per rate.
fn with_rates(c: Computed, limited: &[RateTotal]) -> ApiResult<Computed> {
    let tax = limited
        .iter()
        .map(|r| r.tax.minor())
        .try_fold(0i64, i64::checked_add);
    let tax = tax.ok_or(ApiError::Validation("amount_overflow"))?;
    Ok(Computed {
        subtotal: c.subtotal,
        tax,
        total: c
            .subtotal
            .checked_add(tax)
            .ok_or(ApiError::Validation("amount_overflow"))?,
        per_rate: limited
            .iter()
            .map(|r| RateTotalView {
                rate_bps: r.rate_bps,
                base_minor: r.base.minor(),
                tax_minor: r.tax.minor(),
            })
            .collect(),
    })
}

async fn write_lines(tx: &mut Tx, org_id: Uuid, id: Uuid, lines: &[LineInput]) -> ApiResult<()> {
    sqlx::query("DELETE FROM issued_invoice_lines WHERE invoice_id = $1")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    for (i, l) in lines.iter().enumerate() {
        sqlx::query(
            "INSERT INTO issued_invoice_lines (id, org_id, invoice_id, position, description,
                                               quantity, unit, unit_price_minor, tax_rate_bps)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(Uuid::now_v7())
        .bind(org_id)
        .bind(id)
        .bind(i32::try_from(i + 1).map_err(|_| ApiError::Validation("too_many_lines"))?)
        .bind(l.description.trim())
        .bind(i32::try_from(l.quantity).map_err(|_| ApiError::Validation("quantity"))?)
        .bind(l.unit.trim())
        .bind(l.unit_price_minor)
        .bind(i32::try_from(l.tax_rate_bps).map_err(|_| ApiError::Validation("tax_rate_bps"))?)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Admin transaction scoped to the customer organisation `org_id` (which must exist and be
/// a customer organisation); also returns the platform id and the issuer's rounding rule.
pub(crate) async fn org_billing_tx(
    state: &AppState,
    p: &Principal,
    org_id: Uuid,
) -> ApiResult<(Tx, Uuid, Option<IssuerProfile>, String)> {
    let (mut tx, platform) = billing_tx(state, p).await?;
    let issuer = load_issuer(&mut tx, platform).await?;
    let org: Option<(String, String)> =
        sqlx::query_as("SELECT name, kind FROM organisations WHERE id = $1")
            .bind(org_id)
            .fetch_optional(&mut *tx)
            .await?;
    let (name, kind) = org.ok_or(ApiError::NotFound)?;
    if kind != "customer" && kind != "consumer" {
        return Err(ApiError::Validation("not_a_customer_org"));
    }
    db::set_org(&mut tx, org_id).await?;
    Ok((tx, platform, issuer, name))
}

async fn audit_both(
    tx: &mut Tx,
    p: &Principal,
    platform: Uuid,
    org_id: Uuid,
    action: &str,
    id: Uuid,
    data: serde_json::Value,
) -> ApiResult<()> {
    for target in [platform, org_id] {
        Audit { org_id: target, p }
            .record(tx, action, "issued_invoice", id, data.clone())
            .await?;
    }
    Ok(())
}

async fn check_request(tx: &mut Tx, request_id: Option<Uuid>) -> ApiResult<()> {
    if let Some(r) = request_id {
        let ok: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM procurement_requests WHERE id = $1 AND org_id = app_org_id())",
        )
        .bind(r)
        .fetch_one(&mut **tx)
        .await?;
        if !ok {
            return Err(ApiError::Validation("request_id"));
        }
    }
    Ok(())
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct CreatedInvoice {
    pub id: Uuid,
}

#[utoipa::path(
    operation_id = "admin_create_invoice",
    post,
    path = "/api/admin/v1/orgs/{org_id}/billing/invoices",
    tag = "billing",
    params(("org_id" = Uuid, Path)),
    request_body = InvoiceInput,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Draft created (totals computed by the server)", body = CreatedInvoice),
        (status = 422, description = "Invalid lines, rate, currency or recipient", body = ErrorBody),
        (status = 404, description = "Unknown organisation", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn create_invoice(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<InvoiceInput>,
) -> ApiResult<(StatusCode, Json<CreatedInvoice>)> {
    let (mut tx, platform, issuer, org_name) = org_billing_tx(&state, &p, org_id).await?;
    let id = insert_draft(&mut tx, &p, platform, org_id, &org_name, issuer, &body).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(CreatedInvoice { id })))
}

/// Writes a draft invoice for `org_id` in an `org_billing_tx` transaction and audits it.
pub(crate) async fn insert_draft(
    tx: &mut Tx,
    p: &Principal,
    platform: Uuid,
    org_id: Uuid,
    org_name: &str,
    issuer: Option<IssuerProfile>,
    body: &InvoiceInput,
) -> ApiResult<Uuid> {
    let currency = common::currency(body.currency.as_deref().unwrap_or("JPY"))?;
    let rounding_code = issuer.map_or_else(|| "floor".to_string(), |i| i.tax_rounding);
    let c = compute(&body.lines, currency, rounding(&rounding_code)?, false)?;
    let recipient = common::required(
        body.recipient_name.as_deref().unwrap_or(org_name),
        200,
        false,
        "recipient_name",
    )?;
    let address = common::text(&body.recipient_address, 500, true, "recipient_address")?;
    let notes = common::text(&body.notes, 2000, true, "notes")?;
    check_request(tx, body.request_id).await?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO issued_invoices (id, org_id, kind, currency, recipient_name, recipient_address,
             due_date, request_id, notes, tax_rounding, subtotal_minor, tax_minor, total_minor,
             per_rate, created_by)
         VALUES ($1, $2, 'invoice', $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
    )
    .bind(id)
    .bind(org_id)
    .bind(currency.code())
    .bind(&recipient)
    .bind(&address)
    .bind(body.due_date)
    .bind(body.request_id)
    .bind(&notes)
    .bind(&rounding_code)
    .bind(c.subtotal)
    .bind(c.tax)
    .bind(c.total)
    .bind(serde_json::to_value(&c.per_rate).map_err(ApiError::internal)?)
    .bind(p.user_id)
    .execute(&mut **tx)
    .await?;
    write_lines(tx, org_id, id, &body.lines).await?;
    audit_both(
        tx,
        p,
        platform,
        org_id,
        "billing.draft_created",
        id,
        json!({ "kind": "invoice", "total_minor": c.total, "currency": currency.code() }),
    )
    .await?;
    Ok(id)
}

#[derive(sqlx::FromRow)]
struct DocRow {
    kind: String,
    status: String,
    currency: String,
    original_id: Option<Uuid>,
    tax_rounding: String,
    prices_include_tax: bool,
}

async fn lock_doc(tx: &mut Tx, id: Uuid) -> ApiResult<DocRow> {
    sqlx::query_as::<_, DocRow>(
        "SELECT kind, status, currency, original_id, tax_rounding, prices_include_tax
         FROM issued_invoices WHERE id = $1 AND org_id = app_org_id() FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)
}

#[utoipa::path(
    operation_id = "admin_update_invoice",
    put,
    path = "/api/admin/v1/orgs/{org_id}/billing/invoices/{id}",
    tag = "billing",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = InvoiceInput,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Draft replaced"),
        (status = 409, description = "Already issued (immutable)", body = ErrorBody),
        (status = 422, description = "Invalid input", body = ErrorBody),
        (status = 404, description = "Unknown document", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn update_invoice(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<InvoiceInput>,
) -> ApiResult<StatusCode> {
    let (mut tx, platform, issuer, _) = org_billing_tx(&state, &p, org_id).await?;
    let doc = lock_doc(&mut tx, id).await?;
    if doc.status != "draft" {
        return Err(ApiError::Conflict("already_issued"));
    }
    if doc.kind != "invoice" {
        return Err(ApiError::Validation("kind"));
    }
    let currency = common::currency(body.currency.as_deref().unwrap_or("JPY"))?;
    let rounding_code = issuer.map_or_else(|| "floor".to_string(), |i| i.tax_rounding);
    let c = compute(
        &body.lines,
        currency,
        rounding(&rounding_code)?,
        doc.prices_include_tax,
    )?;
    // Absent: keep the draft's current recipient name (never silently reset it).
    let current: String =
        sqlx::query_scalar("SELECT recipient_name FROM issued_invoices WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    let recipient = common::required(
        body.recipient_name.as_deref().unwrap_or(&current),
        200,
        false,
        "recipient_name",
    )?;
    let address = common::text(&body.recipient_address, 500, true, "recipient_address")?;
    let notes = common::text(&body.notes, 2000, true, "notes")?;
    check_request(&mut tx, body.request_id).await?;
    sqlx::query(
        "UPDATE issued_invoices SET currency = $2, recipient_name = $3, recipient_address = $4,
             due_date = $5, request_id = $6, notes = $7, tax_rounding = $8, subtotal_minor = $9,
             tax_minor = $10, total_minor = $11, per_rate = $12, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(currency.code())
    .bind(&recipient)
    .bind(&address)
    .bind(body.due_date)
    .bind(body.request_id)
    .bind(&notes)
    .bind(&rounding_code)
    .bind(c.subtotal)
    .bind(c.tax)
    .bind(c.total)
    .bind(serde_json::to_value(&c.per_rate).map_err(ApiError::internal)?)
    .execute(&mut *tx)
    .await?;
    write_lines(&mut tx, org_id, id, &body.lines).await?;
    audit_both(
        &mut tx,
        &p,
        platform,
        org_id,
        "billing.draft_updated",
        id,
        json!({ "total_minor": c.total }),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    operation_id = "admin_delete_invoice",
    delete,
    path = "/api/admin/v1/orgs/{org_id}/billing/invoices/{id}",
    tag = "billing",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Draft deleted"),
        (status = 409, description = "Already issued (never deleted)", body = ErrorBody),
        (status = 404, description = "Unknown document", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn delete_invoice(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    let (mut tx, platform, _, _) = org_billing_tx(&state, &p, org_id).await?;
    let doc = lock_doc(&mut tx, id).await?;
    if doc.status != "draft" {
        return Err(ApiError::Conflict("already_issued"));
    }
    sqlx::query("DELETE FROM issued_invoices WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    audit_both(
        &mut tx,
        &p,
        platform,
        org_id,
        "billing.draft_deleted",
        id,
        json!({ "kind": doc.kind }),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

fn per_rate_of(value: &serde_json::Value, currency: Currency) -> ApiResult<Vec<RateTotal>> {
    let views: Vec<RateTotalView> =
        serde_json::from_value(value.clone()).map_err(ApiError::internal)?;
    Ok(views
        .into_iter()
        .map(|v| RateTotal {
            rate_bps: v.rate_bps,
            base: Money::from_minor(v.base_minor, currency),
            tax: Money::from_minor(v.tax_minor, currency),
        })
        .collect())
}

/// Credited so far against `original_id` (issued credit notes only), excluding `except`.
async fn credited(
    tx: &mut Tx,
    original_id: Uuid,
    except: Option<Uuid>,
    currency: Currency,
) -> ApiResult<Vec<RateTotal>> {
    let rows: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT per_rate FROM issued_invoices
         WHERE original_id = $1 AND status <> 'draft' AND id IS DISTINCT FROM $2",
    )
    .bind(original_id)
    .bind(except)
    .fetch_all(&mut **tx)
    .await?;
    let mut out = Vec::new();
    for r in rows {
        out.extend(per_rate_of(&r, currency)?);
    }
    Ok(out)
}

#[utoipa::path(
    operation_id = "admin_create_credit_note",
    post,
    path = "/api/admin/v1/orgs/{org_id}/billing/invoices/{id}/credit-notes",
    tag = "billing",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = CreditNoteInput,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Draft credit note created", body = CreatedInvoice),
        (status = 409, description = "Original not issued, or more than was invoiced at a rate", body = ErrorBody),
        (status = 422, description = "Invalid input", body = ErrorBody),
        (status = 404, description = "Unknown invoice", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn create_credit_note(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath((org_id, original_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<CreditNoteInput>,
) -> ApiResult<(StatusCode, Json<CreatedInvoice>)> {
    let (mut tx, platform, _, _) = org_billing_tx(&state, &p, org_id).await?;
    let reason = common::required(&body.reason, 2000, true, "reason")?;
    let notes = common::text(&body.notes, 2000, true, "notes")?;
    let id = credit_in_tx(
        &mut tx,
        &p,
        platform,
        org_id,
        original_id,
        &reason,
        &notes,
        &body.lines,
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(CreatedInvoice { id })))
}

/// A draft tax-inclusive invoice (consumer shop) inside the caller's platform-scoped
/// transaction for consumer organisation `org_id`. Returns its id.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn shop_invoice_in_tx(
    tx: &mut Tx,
    p: &Principal,
    platform: Uuid,
    org_id: Uuid,
    recipient_name: &str,
    recipient_address: &str,
    due_date: NaiveDate,
    notes: &str,
    lines: &[LineInput],
    rounding_code: &str,
) -> ApiResult<Uuid> {
    let currency = Currency::Jpy;
    let c = compute(lines, currency, rounding(rounding_code)?, true)?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO issued_invoices (id, org_id, kind, currency, recipient_name, recipient_address,
             due_date, notes, tax_rounding, subtotal_minor, tax_minor, total_minor, per_rate,
             created_by, prices_include_tax)
         VALUES ($1, $2, 'invoice', $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, true)",
    )
    .bind(id)
    .bind(org_id)
    .bind(currency.code())
    .bind(recipient_name)
    .bind(recipient_address)
    .bind(due_date)
    .bind(notes)
    .bind(rounding_code)
    .bind(c.subtotal)
    .bind(c.tax)
    .bind(c.total)
    .bind(serde_json::to_value(&c.per_rate).map_err(ApiError::internal)?)
    .bind(p.user_id)
    .execute(&mut **tx)
    .await?;
    write_lines(tx, org_id, id, lines).await?;
    audit_both(
        tx,
        p,
        platform,
        org_id,
        "billing.draft_created",
        id,
        json!({ "kind": "invoice", "source": "shop", "total_minor": c.total }),
    )
    .await?;
    Ok(id)
}

/// A draft credit note against `original_id` inside the caller's platform-scoped
/// transaction (limits per rate checked). Returns its id.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn credit_in_tx(
    tx: &mut Tx,
    p: &Principal,
    platform: Uuid,
    org_id: Uuid,
    original_id: Uuid,
    reason: &str,
    notes: &str,
    lines: &[LineInput],
) -> ApiResult<Uuid> {
    let original = lock_doc(tx, original_id).await?;
    if original.kind != "invoice" || original.status == "draft" {
        return Err(ApiError::Conflict("original_not_issued"));
    }
    let currency = common::currency(&original.currency)?;
    let c = compute(
        lines,
        currency,
        rounding(&original.tax_rounding)?,
        original.prices_include_tax,
    )?;
    // Checked again at issue (other credit notes may be issued in between).
    let orig_rates: serde_json::Value =
        sqlx::query_scalar("SELECT per_rate FROM issued_invoices WHERE id = $1")
            .bind(original_id)
            .fetch_one(&mut **tx)
            .await?;
    let already = credited(tx, original_id, None, currency).await?;
    let limited = invoicing::limit_credit(
        &per_rate_of(&orig_rates, currency)?,
        &already,
        &per_rate_of(
            &serde_json::to_value(&c.per_rate).map_err(ApiError::internal)?,
            currency,
        )?,
    )
    .map_err(invoice_error)?;
    let c = with_rates(c, &limited)?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO issued_invoices (id, org_id, kind, original_id, currency, recipient_name,
             recipient_address, request_id, notes, reason, tax_rounding, subtotal_minor, tax_minor,
             total_minor, per_rate, created_by, prices_include_tax)
         SELECT $1, org_id, 'credit_note', id, currency, recipient_name, recipient_address,
                request_id, $3, $4, tax_rounding, $5, $6, $7, $8, $9, prices_include_tax
         FROM issued_invoices WHERE id = $2",
    )
    .bind(id)
    .bind(original_id)
    .bind(notes)
    .bind(reason)
    .bind(c.subtotal)
    .bind(c.tax)
    .bind(c.total)
    .bind(serde_json::to_value(&c.per_rate).map_err(ApiError::internal)?)
    .bind(p.user_id)
    .execute(&mut **tx)
    .await?;
    write_lines(tx, org_id, id, lines).await?;
    audit_both(
        tx,
        p,
        platform,
        org_id,
        "billing.draft_created",
        id,
        json!({ "kind": "credit_note", "original": original_id, "total_minor": c.total }),
    )
    .await?;
    Ok(id)
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct IssueRequest {
    /// Default: today (Japan time). Not in the future, not before the original invoice.
    pub issue_date: Option<NaiveDate>,
}

#[derive(sqlx::FromRow)]
struct IssueRow {
    recipient_name: String,
    recipient_address: String,
    due_date: Option<NaiveDate>,
    notes: String,
    reason: String,
}

fn view_lines(rows: &[LineView], currency: Currency) -> ApiResult<Vec<LineInput>> {
    rows.iter()
        .map(|l| {
            Ok(LineInput {
                description: l.description.clone(),
                quantity: u32::try_from(l.quantity).map_err(ApiError::internal)?,
                unit: l.unit.clone(),
                unit_price_minor: l.unit_price_minor,
                tax_rate_bps: u32::try_from(l.tax_rate_bps).map_err(ApiError::internal)?,
            })
        })
        .collect::<ApiResult<Vec<_>>>()
        .and_then(|v| {
            // Same validation as on save.
            domain_lines(&v, currency)?;
            Ok(v)
        })
}

async fn load_lines(tx: &mut Tx, id: Uuid) -> ApiResult<Vec<LineView>> {
    Ok(sqlx::query_as::<_, LineView>(
        "SELECT position, description, quantity, unit, unit_price_minor, tax_rate_bps
         FROM issued_invoice_lines WHERE invoice_id = $1 ORDER BY position",
    )
    .bind(id)
    .fetch_all(&mut **tx)
    .await?)
}

#[utoipa::path(
    operation_id = "admin_issue_invoice",
    post,
    path = "/api/admin/v1/orgs/{org_id}/billing/invoices/{id}/issue",
    tag = "billing",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = IssueRequest,
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Issued: number assigned, PDF stored, immutable from now on", body = InvoiceDetail),
        (status = 409, description = "Already issued, issuer not set up, PDF font missing, or credit exceeds the original", body = ErrorBody),
        (status = 422, description = "Invalid issue date", body = ErrorBody),
        (status = 404, description = "Unknown document", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn issue(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<IssueRequest>,
) -> ApiResult<Json<InvoiceDetail>> {
    let (mut tx, platform, issuer, _) = org_billing_tx(&state, &p, org_id).await?;
    let issuer = issuer.ok_or(ApiError::Conflict("issuer_not_configured"))?;
    let detail = issue_in_tx(
        &state,
        &mut tx,
        &p,
        platform,
        org_id,
        &issuer,
        id,
        body.issue_date,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(detail))
}

/// Issues draft `id` inside the caller's platform-scoped transaction (app.org_id = `org_id`):
/// next number, PDF, immutable from then on. Used by the billing screen and by the shop.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn issue_in_tx(
    state: &AppState,
    tx: &mut Tx,
    p: &Principal,
    platform: Uuid,
    org_id: Uuid,
    issuer: &IssuerProfile,
    id: Uuid,
    issue_date: Option<NaiveDate>,
) -> ApiResult<InvoiceDetail> {
    let font = state
        .pdf_font
        .clone()
        .ok_or(ApiError::Conflict("pdf_unavailable"))?;
    let doc = lock_doc(tx, id).await?;
    if doc.status != "draft" {
        return Err(ApiError::Conflict("already_issued"));
    }
    let currency = common::currency(&doc.currency)?;
    let today = today_jst();
    let issue_date = issue_date.unwrap_or(today);
    if issue_date > today {
        return Err(ApiError::Validation("issue_date"));
    }
    let row = sqlx::query_as::<_, IssueRow>(
        "SELECT recipient_name, recipient_address, due_date, notes, reason
         FROM issued_invoices WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    let line_rows = load_lines(tx, id).await?;
    let lines = view_lines(&line_rows, currency)?;
    // Invoices: the issuer's current rounding rule (the one in the printed snapshot); credit
    // notes keep the original invoice's rule. Recomputed from the lines either way.
    let rounding_code = if doc.kind == "invoice" {
        issuer.tax_rounding.clone()
    } else {
        doc.tax_rounding.clone()
    };
    let mut c = compute(
        &lines,
        currency,
        rounding(&rounding_code)?,
        doc.prices_include_tax,
    )?;

    let mut original_ref = None;
    if let Some(orig) = doc.original_id {
        let o = lock_doc(tx, orig).await?;
        if o.status == "draft" {
            return Err(ApiError::Conflict("original_not_issued"));
        }
        let (number, date, rates): (String, NaiveDate, serde_json::Value) = sqlx::query_as(
            "SELECT number, issue_date, per_rate FROM issued_invoices WHERE id = $1",
        )
        .bind(orig)
        .fetch_one(&mut **tx)
        .await?;
        if issue_date < date {
            return Err(ApiError::Validation("issue_date"));
        }
        let already = credited(tx, orig, Some(id), currency).await?;
        let limited = invoicing::limit_credit(
            &per_rate_of(&rates, currency)?,
            &already,
            &per_rate_of(
                &serde_json::to_value(&c.per_rate).map_err(ApiError::internal)?,
                currency,
            )?,
        )
        .map_err(invoice_error)?;
        c = with_rates(c, &limited)?;
        original_ref = Some((number, date.to_string()));
    }

    // Numbers follow dates: no document may be dated before the last one in its series.
    let latest: Option<NaiveDate> = sqlx::query_scalar(
        "SELECT max(issue_date) FROM issued_invoices
         WHERE kind = $1 AND status <> 'draft' AND extract(year FROM issue_date) = $2",
    )
    .bind(&doc.kind)
    .bind(f64::from(issue_date.year()))
    .fetch_one(&mut **tx)
    .await?;
    if latest.is_some_and(|l| issue_date < l) {
        return Err(ApiError::Validation("issue_date"));
    }

    let due_date = if doc.kind == "invoice" {
        let due = row
            .due_date
            .unwrap_or(issue_date + Duration::days(i64::from(issuer.payment_terms_days)));
        if due < issue_date {
            return Err(ApiError::Validation("due_date"));
        }
        Some(due)
    } else {
        None
    };

    // Gap-free: the counter row stays locked until this transaction commits or rolls back.
    let seq: i32 = sqlx::query_scalar(
        "INSERT INTO invoice_counters (kind, year, last) VALUES ($1, $2, 1)
         ON CONFLICT (kind, year) DO UPDATE SET last = invoice_counters.last + 1
         RETURNING last",
    )
    .bind(&doc.kind)
    .bind(issue_date.year())
    .fetch_one(&mut **tx)
    .await?;
    let number = if doc.kind == "invoice" {
        format!("{}-{}-{seq:05}", issuer.number_prefix, issue_date.year())
    } else {
        format!("{}-CN-{}-{seq:05}", issuer.number_prefix, issue_date.year())
    };

    let pdf_doc = invoice_pdf::PdfDocument {
        credit_note: doc.kind == "credit_note",
        tax_inclusive: doc.prices_include_tax,
        number: number.clone(),
        issue_date: issue_date.to_string(),
        due_date: due_date.map(|d| d.to_string()),
        original: original_ref,
        recipient: invoice_pdf::Party {
            name: row.recipient_name,
            address: row.recipient_address,
        },
        issuer: invoice_pdf::Issuer {
            legal_name: issuer.legal_name.clone(),
            registration_number: issuer.registration_number.clone(),
            address: issuer.address.clone(),
            phone: issuer.phone.clone(),
            email: issuer.email.clone(),
            bank_details: issuer.bank_details.clone(),
        },
        lines: lines
            .iter()
            .map(|l| invoice_pdf::PdfLine {
                description: l.description.clone(),
                quantity: l.quantity,
                unit: l.unit.clone(),
                unit_price: Money::from_minor(l.unit_price_minor, currency),
                tax_rate_bps: l.tax_rate_bps,
                amount: Money::from_minor(
                    l.unit_price_minor.saturating_mul(i64::from(l.quantity)),
                    currency,
                ),
            })
            .collect(),
        per_rate: c
            .per_rate
            .iter()
            .map(|r| invoice_pdf::PdfRate {
                rate_bps: r.rate_bps,
                base: Money::from_minor(r.base_minor, currency),
                tax: Money::from_minor(r.tax_minor, currency),
            })
            .collect(),
        subtotal: Money::from_minor(c.subtotal, currency),
        tax: Money::from_minor(c.tax, currency),
        total: Money::from_minor(c.total, currency),
        notes: row.notes,
        reason: row.reason,
    };
    let pdf = tokio::task::spawn_blocking(move || invoice_pdf::render(&font, &pdf_doc))
        .await
        .map_err(ApiError::internal)?
        .map_err(|e| match e {
            invoice_pdf::RenderError::TextTooLong => ApiError::Validation("text_too_long"),
            invoice_pdf::RenderError::Other(e) => ApiError::internal(std::io::Error::other(e)),
        })?;
    let hash = state.storage.put(&pdf).await.map_err(ApiError::internal)?;
    let issuer_json = serde_json::to_value(issuer).map_err(ApiError::internal)?;
    sqlx::query(
        "UPDATE issued_invoices SET status = 'issued', number = $2, issue_date = $3, due_date = $4,
             issuer = $5, pdf_sha256 = $6, issued_by = $7, issued_at = now(), subtotal_minor = $8,
             tax_minor = $9, total_minor = $10, per_rate = $11, tax_rounding = $12,
             updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(&number)
    .bind(issue_date)
    .bind(due_date)
    .bind(&issuer_json)
    .bind(hash.as_slice())
    .bind(p.user_id)
    .bind(c.subtotal)
    .bind(c.tax)
    .bind(c.total)
    .bind(serde_json::to_value(&c.per_rate).map_err(ApiError::internal)?)
    .bind(&rounding_code)
    .execute(&mut **tx)
    .await?;
    audit_both(
        tx,
        p,
        platform,
        org_id,
        "billing.issued",
        id,
        json!({ "kind": doc.kind, "number": number, "total_minor": c.total,
                "currency": currency.code(), "pdf_sha256": hex(&hash) }),
    )
    .await?;
    load_detail(tx, id, true).await
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct PaymentUpdate {
    /// true: received (needs `paid_on`); false: undo a mistaken payment record.
    pub paid: bool,
    pub paid_on: Option<NaiveDate>,
    #[serde(default)]
    pub note: String,
}

#[utoipa::path(
    operation_id = "admin_record_payment",
    post,
    path = "/api/admin/v1/orgs/{org_id}/billing/invoices/{id}/payment",
    tag = "billing",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = PaymentUpdate,
    security(("bearer" = [])),
    responses(
        (status = 204, description = "Payment status recorded"),
        (status = 409, description = "Not an issued invoice", body = ErrorBody),
        (status = 422, description = "Invalid date", body = ErrorBody),
        (status = 404, description = "Unknown document", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn record_payment(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<PaymentUpdate>,
) -> ApiResult<StatusCode> {
    let note = common::text(&body.note, 500, false, "note")?;
    let (mut tx, platform, _, _) = org_billing_tx(&state, &p, org_id).await?;
    let doc = lock_doc(&mut tx, id).await?;
    if doc.kind != "invoice" || doc.status == "draft" {
        return Err(ApiError::Conflict("not_issued"));
    }
    if body.paid {
        if doc.status == "paid" {
            return Err(ApiError::Conflict("already_paid"));
        }
        let paid_on = body.paid_on.ok_or(ApiError::Validation("paid_on"))?;
        let issued: NaiveDate =
            sqlx::query_scalar("SELECT issue_date FROM issued_invoices WHERE id = $1")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
        if paid_on < issued || paid_on > today_jst() {
            return Err(ApiError::Validation("paid_on"));
        }
        sqlx::query(
            "UPDATE issued_invoices SET status = 'paid', paid_on = $2, paid_by = $3, paid_note = $4,
                 updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(paid_on)
        .bind(p.user_id)
        .bind(&note)
        .execute(&mut *tx)
        .await?;
    } else {
        if doc.status != "paid" {
            return Err(ApiError::Conflict("not_paid"));
        }
        sqlx::query(
            "UPDATE issued_invoices SET status = 'issued', paid_on = NULL, paid_by = NULL,
                 paid_note = $2, updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(&note)
        .execute(&mut *tx)
        .await?;
    }
    audit_both(
        &mut tx,
        &p,
        platform,
        org_id,
        if body.paid {
            "billing.paid"
        } else {
            "billing.payment_undone"
        },
        id,
        json!({ "paid_on": body.paid_on, "note": note }),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(sqlx::FromRow)]
struct DetailRow {
    recipient_name: String,
    recipient_address: String,
    request_id: Option<Uuid>,
    notes: String,
    reason: String,
    tax_rounding: String,
    subtotal_minor: i64,
    tax_minor: i64,
    per_rate: serde_json::Value,
    paid_on: Option<NaiveDate>,
    paid_note: String,
    pdf_sha256: Option<Vec<u8>>,
}

async fn load_detail(tx: &mut Tx, id: Uuid, admin: bool) -> ApiResult<InvoiceDetail> {
    let summary = sqlx::query_as::<_, InvoiceSummary>(&format!("{SUMMARY_SELECT} WHERE i.id = $1"))
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    let d = sqlx::query_as::<_, DetailRow>(
        "SELECT recipient_name, recipient_address, request_id, notes, reason, tax_rounding,
                subtotal_minor, tax_minor, per_rate, paid_on, paid_note, pdf_sha256
         FROM issued_invoices WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    let lines = load_lines(tx, id).await?;
    let credit_notes = sqlx::query_as::<_, InvoiceSummary>(&format!(
        "{SUMMARY_SELECT} WHERE i.original_id = $1 AND ($2 OR i.status <> 'draft')
         ORDER BY i.created_at"
    ))
    .bind(id)
    .bind(admin)
    .fetch_all(&mut **tx)
    .await?;
    Ok(InvoiceDetail {
        summary,
        recipient_name: d.recipient_name,
        recipient_address: d.recipient_address,
        request_id: d.request_id,
        notes: d.notes,
        reason: d.reason,
        tax_rounding: d.tax_rounding,
        subtotal_minor: d.subtotal_minor,
        tax_minor: d.tax_minor,
        per_rate: serde_json::from_value(d.per_rate).map_err(ApiError::internal)?,
        lines,
        credit_notes,
        paid_on: d.paid_on,
        paid_note: d.paid_note,
        pdf_sha256: d.pdf_sha256.as_deref().map(hex),
    })
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct InvoiceQuery {
    /// Only this organisation.
    pub org_id: Option<Uuid>,
    /// `draft`, `issued`, `paid` or `overdue`.
    pub status: Option<String>,
}

#[utoipa::path(
    operation_id = "admin_list_invoices",
    get,
    path = "/api/admin/v1/billing/invoices",
    tag = "billing",
    params(InvoiceQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Invoices and credit notes of all organisations (newest first, at most 500)", body = [InvoiceSummary]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn list_invoices(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiQuery(q): ApiQuery<InvoiceQuery>,
) -> ApiResult<Json<Vec<InvoiceSummary>>> {
    let status = q.status.as_deref();
    if let Some(s) = status
        && !["draft", "issued", "paid", "overdue"].contains(&s)
    {
        return Err(ApiError::Validation("status"));
    }
    let (mut tx, _) = billing_tx(&state, &p).await?;
    let rows = sqlx::query_as::<_, InvoiceSummary>(&format!(
        "SELECT * FROM ({SUMMARY_SELECT}) s
         WHERE ($1::uuid IS NULL OR s.org_id = $1)
           AND ($2::text IS NULL OR ($2 = 'overdue' AND s.overdue) OR s.status = $2)
         ORDER BY s.created_at DESC LIMIT 500"
    ))
    .bind(q.org_id)
    .bind(status)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[utoipa::path(
    operation_id = "admin_get_invoice",
    get,
    path = "/api/admin/v1/orgs/{org_id}/billing/invoices/{id}",
    tag = "billing",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Document with lines, per-rate totals and credit notes", body = InvoiceDetail),
        (status = 404, description = "Unknown document", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn get_invoice(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<InvoiceDetail>> {
    let (mut tx, _, _, _) = org_billing_tx(&state, &p, org_id).await?;
    let detail = load_detail(&mut tx, id, true).await?;
    if detail.summary.org_id != org_id {
        return Err(ApiError::NotFound);
    }
    tx.commit().await?;
    Ok(Json(detail))
}

pub(crate) async fn pdf_response(
    state: &AppState,
    tx: Tx,
    id: Uuid,
) -> ApiResult<impl IntoResponse + use<>> {
    let mut tx = tx;
    let row: Option<(Option<String>, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT number, pdf_sha256 FROM issued_invoices WHERE id = $1 AND org_id = app_org_id()",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    let (number, sha) = row.ok_or(ApiError::NotFound)?;
    let (Some(number), Some(sha)) = (number, sha) else {
        return Err(ApiError::Conflict("not_issued"));
    };
    let hash: [u8; 32] = sha
        .as_slice()
        .try_into()
        .map_err(|_| ApiError::internal(std::io::Error::other("bad stored hash")))?;
    let bytes = state.storage.get(&hash).await.map_err(ApiError::internal)?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/pdf"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    // Numbers are A–Z, 0–9 and '-', so the header value is always plain ASCII.
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{number}.pdf\""))
            .map_err(ApiError::internal)?,
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok((headers, bytes))
}

#[utoipa::path(
    operation_id = "admin_invoice_pdf",
    get,
    path = "/api/admin/v1/orgs/{org_id}/billing/invoices/{id}/pdf",
    tag = "billing",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "The issued PDF (application/pdf)"),
        (status = 409, description = "Draft (no PDF yet)", body = ErrorBody),
        (status = 404, description = "Unknown document", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid admin session", body = ErrorBody),
    )
)]
pub async fn admin_pdf(
    State(state): State<AppState>,
    AdminPrincipal(p): AdminPrincipal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<impl IntoResponse> {
    let (tx, _, _, _) = org_billing_tx(&state, &p, org_id).await?;
    pdf_response(&state, tx, id).await
}

// ---------------------------------------------------------------------------------------
// Customer side: issued documents only, `view_finance`.

async fn customer_tx(state: &AppState, p: &Principal, org_id: Uuid) -> ApiResult<Tx> {
    let access = p.access(state, org_id).await?;
    // Customer members only: Chuo-Sogo staff use the admin console for billing.
    if !matches!(access, procure_domain::rbac::Access::Member(r) if !r.is_internal()) {
        return Err(ApiError::NotFound);
    }
    // Finance staff of a business customer, or a consumer for their own orders.
    if !rbac_allows(access, Permission::ViewFinance) && !rbac_allows(access, Permission::Shop) {
        return Err(ApiError::Forbidden);
    }
    Ok(db::tenant_tx(&state.pool, p.user_id, org_id).await?)
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/billing/invoices",
    tag = "billing",
    params(("org_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Invoices and credit notes Chuo-Sogo issued to this organisation", body = [InvoiceSummary]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown organisation", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn customer_list(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
) -> ApiResult<Json<Vec<InvoiceSummary>>> {
    let mut tx = customer_tx(&state, &p, org_id).await?;
    // RLS already hides drafts and other organisations; the filter states the intent.
    let rows = sqlx::query_as::<_, InvoiceSummary>(&format!(
        "{SUMMARY_SELECT} WHERE i.org_id = $1 AND i.status <> 'draft'
         ORDER BY i.issue_date DESC, i.number DESC LIMIT 500"
    ))
    .bind(org_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/billing/invoices/{id}",
    tag = "billing",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Issued document", body = InvoiceDetail),
        (status = 404, description = "Unknown, draft or not visible", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn customer_get(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<InvoiceDetail>> {
    let mut tx = customer_tx(&state, &p, org_id).await?;
    let detail = load_detail(&mut tx, id, false).await?;
    if detail.summary.org_id != org_id || detail.summary.status == "draft" {
        return Err(ApiError::NotFound);
    }
    tx.commit().await?;
    Ok(Json(detail))
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/billing/invoices/{id}/pdf",
    tag = "billing",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "The issued PDF (application/pdf)"),
        (status = 404, description = "Unknown, draft or not visible", body = ErrorBody),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn customer_pdf(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<impl IntoResponse> {
    let tx = customer_tx(&state, &p, org_id).await?;
    pdf_response(&state, tx, id).await
}
