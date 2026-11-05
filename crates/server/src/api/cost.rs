//! Should-cost models and negotiation briefs (L5, docs/agents/DESIGN.md).
//!
//! - Cost models: per item and version, what one unit ought to cost
//!   (`procure_domain::should_cost`). Saving one adds a version; versions never change.
//! - Quotation facts: the deterministic comparison of one supplier's quotation with the
//!   factory's own prices, the should-cost and the other quotations. The `cost_analyst`
//!   agent uses the same facts to propose a negotiation.
//! - Negotiation briefs: the facts of one quotation, an optional AI narrative drafted from
//!   those facts only (stored as an `ai_suggestions` row), and the text a person finalises.
//!   Nothing here changes a quotation, a comparison or a recommendation.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, Datelike, Utc};
use procure_ai::negotiation::{self, NEGOTIATION_BRIEF};
use procure_domain::aliases;
use procure_domain::history::{self, Day};
use procure_domain::money::Currency;
use procure_domain::rbac::{self, Access, Permission};
use procure_domain::should_cost::{
    self as sc, CostInputs, LineInput, Material, PastPrice, Process, Thresholds,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::AppState;
use crate::api::ai::{self, NewSuggestion, SuggestionView};
use crate::api::common::{self, Audit};
use crate::db::{self, Tx};
use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ApiResult, ErrorBody};
use crate::principal::Principal;

/// Past purchases read per item for the facts (newest first).
const HISTORY_DEPTH: i64 = 200;

/// Reading: anyone who may manage the purchase records or see all requests.
async fn require_read(state: &AppState, p: &Principal, org_id: Uuid) -> ApiResult<Access> {
    let access = p.access(state, org_id).await?;
    if access == Access::None {
        return Err(ApiError::NotFound);
    }
    if rbac::allows(access, Permission::ManagePurchaseHistory)
        || rbac::allows(access, Permission::ViewAllRequests)
    {
        Ok(access)
    } else {
        Err(ApiError::Forbidden)
    }
}

/// Writing models and briefs: those who manage the organisation's price records.
async fn require_write(state: &AppState, p: &Principal, org_id: Uuid) -> ApiResult<Access> {
    p.require(state, org_id, Permission::ManagePurchaseHistory)
        .await
}

fn internal(what: &'static str) -> ApiError {
    ApiError::internal(std::io::Error::other(what))
}

// ---------------------------------------------------------------------------------------
// Cost models

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MaterialInput {
    pub name: String,
    /// Unit the quantity and price refer to (`kg`, `g`, `m`, `pcs`, ...), for people.
    #[serde(default)]
    pub unit: String,
    /// Thousandths of the unit per piece (250 = 0.25 kg).
    pub quantity_milli: i64,
    /// Price per whole unit, minor currency units.
    pub unit_price_minor: i64,
    /// Material lost in production, basis points (1000 = 10 %).
    #[serde(default)]
    pub scrap_bps: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessInput {
    pub name: String,
    /// Machine time per piece, milliseconds.
    pub cycle_ms: i64,
    /// Machine and operator rate per hour, minor currency units.
    pub rate_minor_per_hour: i64,
    /// Set-up once per lot, minutes.
    #[serde(default)]
    pub setup_minutes: u32,
}

/// The inputs of a cost model (stored as given, after validation).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelInputs {
    /// Pieces per production lot (set-up and logistics are spread over it).
    pub lot_size: u32,
    #[serde(default)]
    pub materials: Vec<MaterialInput>,
    #[serde(default)]
    pub processes: Vec<ProcessInput>,
    /// On material + processes, basis points.
    #[serde(default)]
    pub overhead_bps: u32,
    /// Transport and packing per lot, minor currency units.
    #[serde(default)]
    pub logistics_minor_per_lot: i64,
    /// On everything above, basis points.
    #[serde(default)]
    pub margin_bps: u32,
}

impl ModelInputs {
    fn domain(&self) -> CostInputs {
        CostInputs {
            lot_size: self.lot_size,
            materials: self
                .materials
                .iter()
                .map(|m| Material {
                    name: m.name.clone(),
                    quantity_milli: m.quantity_milli,
                    unit_price_minor: m.unit_price_minor,
                    scrap_bps: m.scrap_bps,
                })
                .collect(),
            processes: self
                .processes
                .iter()
                .map(|p| Process {
                    name: p.name.clone(),
                    cycle_ms: p.cycle_ms,
                    rate_minor_per_hour: p.rate_minor_per_hour,
                    setup_minutes: p.setup_minutes,
                })
                .collect(),
            overhead_bps: self.overhead_bps,
            logistics_minor_per_lot: self.logistics_minor_per_lot,
            margin_bps: self.margin_bps,
        }
    }

    /// Trimmed names, checked bounds (`procure_domain::should_cost::validate`).
    fn checked(mut self) -> ApiResult<Self> {
        for m in &mut self.materials {
            m.name = m.name.trim().to_string();
            m.unit = common::text(&m.unit, 16, false, "unit")?;
        }
        for p in &mut self.processes {
            p.name = p.name.trim().to_string();
        }
        sc::validate(&self.domain()).map_err(|e| ApiError::Validation(e.code()))?;
        Ok(self)
    }
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CostModelInput {
    /// The item, as in a request line: part number, else description (matched like the
    /// purchase records).
    #[serde(default)]
    pub mpn: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub name: String,
    pub currency: String,
    #[serde(default)]
    pub notes: String,
    pub inputs: ModelInputs,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct CostLineView {
    /// `material` | `process`
    pub kind: String,
    pub name: String,
    pub milli: i64,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct BreakdownView {
    pub material_milli: i64,
    pub process_milli: i64,
    pub overhead_milli: i64,
    pub logistics_milli: i64,
    pub margin_milli: i64,
    pub total_milli: i64,
    pub unit_cost_minor: i64,
    pub lines: Vec<CostLineView>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct DriverView {
    /// e.g. `material.0.unit_price`, `process.1.cycle`, `overhead`, `logistics`, `margin`.
    pub input: String,
    pub name: String,
    /// Unit cost increase when this input rises by 10 % (thousandths of a minor unit).
    pub delta_milli: i64,
    pub delta_bps: i64,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct CostModelView {
    pub id: Uuid,
    pub item_key: String,
    pub version: i32,
    pub name: String,
    pub currency: String,
    pub inputs: ModelInputs,
    pub unit_cost_milli: i64,
    pub notes: String,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub breakdown: BreakdownView,
    /// Inputs that drive the unit cost most, largest first.
    pub drivers: Vec<DriverView>,
}

#[derive(sqlx::FromRow)]
struct ModelRow {
    id: Uuid,
    item_key: String,
    version: i32,
    name: String,
    currency: String,
    inputs: Value,
    unit_cost_milli: i64,
    notes: String,
    created_by: Uuid,
    created_at: DateTime<Utc>,
}

const MODEL_COLUMNS: &str =
    "id, item_key, version, name, currency, inputs, unit_cost_milli, notes, created_by, created_at";

fn model_view(r: ModelRow) -> ApiResult<CostModelView> {
    let inputs: ModelInputs =
        serde_json::from_value(r.inputs).map_err(|_| internal("stored cost model"))?;
    let domain = inputs.domain();
    let b = sc::build_up(&domain).map_err(|_| internal("stored cost model"))?;
    let drivers = sc::sensitivity(&domain).map_err(|_| internal("stored cost model"))?;
    Ok(CostModelView {
        id: r.id,
        item_key: r.item_key,
        version: r.version,
        name: r.name,
        currency: r.currency,
        inputs,
        unit_cost_milli: r.unit_cost_milli,
        notes: r.notes,
        created_by: r.created_by,
        created_at: r.created_at,
        breakdown: BreakdownView {
            material_milli: b.material_milli,
            process_milli: b.process_milli,
            overhead_milli: b.overhead_milli,
            logistics_milli: b.logistics_milli,
            margin_milli: b.margin_milli,
            total_milli: b.total_milli,
            unit_cost_minor: b.unit_cost_minor,
            lines: b
                .lines
                .into_iter()
                .map(|l| CostLineView {
                    kind: l.kind.to_string(),
                    name: l.name,
                    milli: l.milli,
                })
                .collect(),
        },
        drivers: drivers
            .into_iter()
            .map(|d| DriverView {
                input: d.input,
                name: d.name,
                delta_milli: d.delta_milli,
                delta_bps: d.delta_bps,
            })
            .collect(),
    })
}

#[utoipa::path(
    operation_id = "cost_create_model",
    post,
    path = "/api/v1/orgs/{org_id}/cost/models",
    tag = "cost",
    params(("org_id" = Uuid, Path)),
    request_body = CostModelInput,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "New version of the item's cost model, with its build-up", body = CostModelView),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 409, description = "Another version was saved at the same time", body = ErrorBody),
        (status = 422, description = "Invalid input (the detail names the field)", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn create_model(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CostModelInput>,
) -> ApiResult<(StatusCode, Json<CostModelView>)> {
    require_write(&state, &p, org_id).await?;
    let currency = common::currency(&body.currency)?;
    let mpn = common::text(&body.mpn, 100, false, "mpn")?;
    let description = common::text(&body.description, 1000, false, "description")?;
    let item_key = history::item_key(&mpn, &description).ok_or(ApiError::Validation("item"))?;
    let name = common::text(&body.name, 200, false, "name")?;
    let notes = common::text(&body.notes, 2000, true, "notes")?;
    let inputs = body.inputs.checked()?;
    let breakdown = sc::build_up(&inputs.domain()).map_err(|e| ApiError::Validation(e.code()))?;
    let stored = serde_json::to_value(&inputs).map_err(ApiError::internal)?;

    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let version: i32 = sqlx::query_scalar(
        "SELECT coalesce(max(version), 0) + 1 FROM cost_models WHERE org_id = $1 AND item_key = $2",
    )
    .bind(org_id)
    .bind(&item_key)
    .fetch_one(&mut *tx)
    .await?;
    let id = Uuid::now_v7();
    let row = sqlx::query_as::<_, ModelRow>(&format!(
        "INSERT INTO cost_models (id, org_id, item_key, version, name, currency, inputs,
                                  unit_cost_milli, notes, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         RETURNING {MODEL_COLUMNS}"
    ))
    .bind(id)
    .bind(org_id)
    .bind(&item_key)
    .bind(version)
    .bind(&name)
    .bind(currency.code())
    .bind(&stored)
    .bind(breakdown.total_milli)
    .bind(&notes)
    .bind(p.user_id)
    .fetch_one(&mut *tx)
    .await?;
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "cost_model.saved",
            "cost_model",
            id,
            json!({ "item_key": item_key, "version": version, "currency": currency.code(),
                    "unit_cost_milli": breakdown.total_milli }),
        )
        .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(model_view(row)?)))
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ModelsQuery {
    /// Every version of this item (newest first); without it, the latest version of each item.
    pub item_key: Option<String>,
}

#[utoipa::path(
    operation_id = "cost_list_models",
    get,
    path = "/api/v1/orgs/{org_id}/cost/models",
    tag = "cost",
    params(("org_id" = Uuid, Path), ModelsQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Cost models", body = [CostModelView]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list_models(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiQuery(q): ApiQuery<ModelsQuery>,
) -> ApiResult<Json<Vec<CostModelView>>> {
    require_read(&state, &p, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let rows = match &q.item_key {
        Some(k) => {
            sqlx::query_as::<_, ModelRow>(&format!(
                "SELECT {MODEL_COLUMNS} FROM cost_models WHERE item_key = $1
                 ORDER BY version DESC LIMIT 500"
            ))
            .bind(k)
            .fetch_all(&mut *tx)
            .await?
        }
        None => {
            sqlx::query_as::<_, ModelRow>(&format!(
                "SELECT {MODEL_COLUMNS} FROM (
                     SELECT DISTINCT ON (item_key) * FROM cost_models
                     ORDER BY item_key, version DESC) m
                 ORDER BY created_at DESC LIMIT 500"
            ))
            .fetch_all(&mut *tx)
            .await?
        }
    };
    tx.commit().await?;
    Ok(Json(
        rows.into_iter().map(model_view).collect::<ApiResult<_>>()?,
    ))
}

#[utoipa::path(
    operation_id = "cost_get_model",
    get,
    path = "/api/v1/orgs/{org_id}/cost/models/{model_id}",
    tag = "cost",
    params(("org_id" = Uuid, Path), ("model_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "One version of a cost model, with build-up and drivers", body = CostModelView),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown model", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn get_model(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, model_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<CostModelView>> {
    require_read(&state, &p, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let row = sqlx::query_as::<_, ModelRow>(&format!(
        "SELECT {MODEL_COLUMNS} FROM cost_models WHERE id = $1"
    ))
    .bind(model_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    tx.commit().await?;
    Ok(Json(model_view(row)?))
}

// ---------------------------------------------------------------------------------------
// Quotation facts

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct LineFactsView {
    pub request_item_id: Uuid,
    pub line_no: i32,
    pub description: String,
    pub item_key: Option<String>,
    pub quantity: u32,
    pub quoted_unit_minor: i64,
    /// The supplier offered a different product: no signals are raised for it.
    pub alternative: bool,
    /// The factory's own purchases of the item in this currency (alias groups included).
    pub purchases: usize,
    pub historical_low_milli: Option<i64>,
    pub last_paid_milli: Option<i64>,
    /// Latest price paid against the earliest, basis points.
    pub trend_bps: Option<i64>,
    /// Quoted unit price against the lowest price paid, basis points.
    pub history_gap_bps: Option<i64>,
    pub cost_model_id: Option<Uuid>,
    pub cost_model_version: Option<i32>,
    pub should_cost_milli: Option<i64>,
    pub should_cost_gap_bps: Option<i64>,
    /// Current quotations for this line in this currency (this one included).
    pub competing_quotes: usize,
    pub competing_low_minor: Option<i64>,
    pub competing_high_minor: Option<i64>,
    pub spread_bps: Option<i64>,
    pub best_reference_milli: Option<i64>,
    pub savings_opportunity_minor: i64,
    /// `above_should_cost`, `above_historical_low`.
    pub signals: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct ThresholdsView {
    pub should_cost_bps: i64,
    /// The organisation's price alert line when set, else the default.
    pub history_bps: i64,
}

/// Deterministic facts about one supplier's quotation (reference only).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct QuotationFacts {
    pub quotation_id: Uuid,
    pub rfq_id: Uuid,
    pub request_id: Uuid,
    pub supplier_id: Uuid,
    pub rfq_number: String,
    pub supplier_name: String,
    pub currency: String,
    /// The quotation was replaced by a later one from the same supplier.
    pub superseded: bool,
    /// Σ unit price × quantity of the quoted lines (shipping excluded).
    pub quoted_total_minor: i64,
    pub shipping_minor: i64,
    pub savings_opportunity_minor: i64,
    pub thresholds: ThresholdsView,
    /// Signals of any line.
    pub signals: Vec<String>,
    pub lines: Vec<LineFactsView>,
}

impl QuotationFacts {
    /// What the narrative is written from: the facts without internal identifiers.
    pub fn narrative_input(&self) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        if let Value::Object(m) = &mut v {
            for k in [
                "quotation_id",
                "rfq_id",
                "request_id",
                "supplier_id",
                "superseded",
            ] {
                m.remove(k);
            }
            if let Some(Value::Array(lines)) = m.get_mut("lines") {
                for l in lines {
                    if let Value::Object(lm) = l {
                        lm.remove("request_item_id");
                        lm.remove("cost_model_id");
                        lm.remove("item_key");
                    }
                }
            }
        }
        v
    }
}

#[derive(sqlx::FromRow)]
struct QuoteHead {
    id: Uuid,
    rfq_id: Uuid,
    rfq_recipient_id: Uuid,
    request_id: Uuid,
    rfq_number: String,
    supplier_id: Uuid,
    supplier_name: String,
    currency: String,
    shipping_minor: i64,
    superseded: bool,
}

#[derive(sqlx::FromRow)]
struct QuoteLine {
    request_item_id: Uuid,
    line_no: i32,
    description: String,
    mpn: String,
    verified_mpn: Option<String>,
    unit_price_minor: i64,
    quantity: i32,
    alternative: bool,
}

/// The facts of quotation `quotation_id` (transaction in its organisation's context).
/// `None` when it does not exist there.
pub(crate) async fn quotation_facts(
    tx: &mut Tx,
    quotation_id: Uuid,
) -> ApiResult<Option<QuotationFacts>> {
    let Some(head) = sqlx::query_as::<_, QuoteHead>(
        "SELECT q.id, q.rfq_id, q.rfq_recipient_id, f.request_id, f.number AS rfq_number,
                rr.supplier_id, s.name AS supplier_name, q.currency, q.shipping_minor,
                q.superseded_at IS NOT NULL AS superseded
         FROM quotations q
         JOIN rfqs f ON f.id = q.rfq_id
         JOIN rfq_recipients rr ON rr.id = q.rfq_recipient_id
         JOIN suppliers s ON s.id = rr.supplier_id
         WHERE q.id = $1",
    )
    .bind(quotation_id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(None);
    };
    let currency = Currency::from_code(&head.currency).ok_or_else(|| internal("currency"))?;
    let lines = sqlx::query_as::<_, QuoteLine>(
        "SELECT ql.request_item_id, ri.line_no, ri.description, ri.mpn, ri.verified_mpn,
                ql.unit_price_minor, ql.quantity, ql.alternative
         FROM quotation_lines ql JOIN request_items ri ON ri.id = ql.request_item_id
         WHERE ql.quotation_id = $1
         ORDER BY ri.line_no, ql.id",
    )
    .bind(quotation_id)
    .fetch_all(&mut **tx)
    .await?;
    let history_bps: Option<i32> = sqlx::query_scalar(
        "SELECT price_alert_bps FROM buying_criteria WHERE org_id = app_org_id()",
    )
    .fetch_optional(&mut **tx)
    .await?
    .flatten();
    let thresholds = Thresholds {
        should_cost_bps: sc::DEFAULT_GAP_THRESHOLD_BPS,
        history_bps: history_bps.map_or(sc::DEFAULT_GAP_THRESHOLD_BPS, i64::from),
    };

    // Item keys and their alias groups.
    let keys: Vec<Option<String>> = lines
        .iter()
        .map(|l| history::item_key(l.verified_mpn.as_deref().unwrap_or(&l.mpn), &l.description))
        .collect();
    let own: Vec<String> = keys.iter().flatten().cloned().collect();
    let pairs: Vec<(String, String)> = sqlx::query_as(
        "SELECT alias_key, canonical_key FROM item_aliases
         WHERE canonical_key = ANY($1)
            OR canonical_key IN (SELECT canonical_key FROM item_aliases WHERE alias_key = ANY($1))
         ORDER BY created_at, id",
    )
    .bind(&own)
    .fetch_all(&mut **tx)
    .await?;
    let groups: BTreeMap<&str, Vec<String>> = own
        .iter()
        .map(|k| (k.as_str(), aliases::key_group(k, &pairs)))
        .collect();
    let wanted: Vec<String> = groups.values().flatten().cloned().collect();

    // Past purchases in the quotation's currency.
    let past: Vec<(String, chrono::NaiveDate, i64)> = sqlx::query_as(
        "SELECT item_key, purchased_on, unit_price_milli FROM (
             SELECT item_key, purchased_on, unit_price_milli,
                    row_number() OVER (PARTITION BY item_key ORDER BY purchased_on DESC, id DESC) AS rn
             FROM purchase_history WHERE item_key = ANY($1) AND currency = $2) h
         WHERE rn <= $3",
    )
    .bind(&wanted)
    .bind(currency.code())
    .bind(HISTORY_DEPTH)
    .fetch_all(&mut **tx)
    .await?;
    let mut past_by_key: BTreeMap<String, Vec<PastPrice>> = BTreeMap::new();
    for (k, d, price) in past {
        let day = Day::new(d.year(), d.month(), d.day()).ok_or_else(|| internal("date"))?;
        past_by_key.entry(k).or_default().push(PastPrice {
            day: day.epoch_days(),
            unit_price_milli: price,
        });
    }

    // Latest cost model per key in this currency.
    let models: Vec<(String, Uuid, i32, i64)> = sqlx::query_as(
        "SELECT DISTINCT ON (item_key) item_key, id, version, unit_cost_milli
         FROM cost_models WHERE item_key = ANY($1) AND currency = $2
         ORDER BY item_key, version DESC",
    )
    .bind(&wanted)
    .bind(currency.code())
    .fetch_all(&mut **tx)
    .await?;
    let models: BTreeMap<String, (Uuid, i32, i64)> = models
        .into_iter()
        .map(|(k, id, v, c)| (k, (id, v, c)))
        .collect();

    // Other suppliers' current quotations for the same lines, same currency.
    let others: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT ql.request_item_id, ql.unit_price_minor
         FROM quotation_lines ql JOIN quotations q ON q.id = ql.quotation_id
         WHERE q.rfq_id = $1 AND q.superseded_at IS NULL AND q.currency = $2
           AND q.rfq_recipient_id <> $3 AND NOT ql.alternative",
    )
    .bind(head.rfq_id)
    .bind(currency.code())
    .bind(head.rfq_recipient_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut others_by_item: BTreeMap<Uuid, Vec<i64>> = BTreeMap::new();
    for (item, price) in others {
        others_by_item.entry(item).or_default().push(price);
    }

    let mut out = Vec::with_capacity(lines.len());
    let mut total: i128 = 0;
    let mut savings: i128 = 0;
    let mut signals: Vec<sc::Signal> = Vec::new();
    for (l, key) in lines.into_iter().zip(keys) {
        let group: &[String] = key
            .as_deref()
            .and_then(|k| groups.get(k))
            .map_or(&[], Vec::as_slice);
        let history: Vec<PastPrice> = group
            .iter()
            .filter_map(|g| past_by_key.get(g))
            .flatten()
            .copied()
            .collect();
        // The item's own model first, else one of its alias group (in key order).
        let model = key
            .as_ref()
            .and_then(|k| models.get(k))
            .or_else(|| group.iter().find_map(|g| models.get(g)))
            .copied();
        let quantity = u32::try_from(l.quantity).map_err(|_| internal("quantity"))?;
        let mut competing = others_by_item
            .get(&l.request_item_id)
            .cloned()
            .unwrap_or_default();
        competing.push(l.unit_price_minor);
        let f = sc::assess_line(
            &LineInput {
                quantity,
                quoted_unit_minor: l.unit_price_minor,
                history,
                should_cost_milli: model.map(|m| m.2),
                competing_minor: competing,
            },
            thresholds,
        );
        let line_signals = if l.alternative { vec![] } else { f.signals };
        for s in &line_signals {
            if !signals.contains(s) {
                signals.push(*s);
            }
        }
        total += i128::from(l.unit_price_minor) * i128::from(quantity);
        let line_savings = if l.alternative {
            0
        } else {
            f.savings_opportunity_minor
        };
        savings += i128::from(line_savings);
        out.push(LineFactsView {
            request_item_id: l.request_item_id,
            line_no: l.line_no,
            description: l.description,
            item_key: key,
            quantity,
            quoted_unit_minor: l.unit_price_minor,
            alternative: l.alternative,
            purchases: f.purchases,
            historical_low_milli: f.historical_low_milli,
            last_paid_milli: f.last_paid_milli,
            trend_bps: f.trend_bps,
            history_gap_bps: f.history_gap_bps,
            cost_model_id: model.map(|m| m.0),
            cost_model_version: model.map(|m| m.1),
            should_cost_milli: f.should_cost_milli,
            should_cost_gap_bps: f.should_cost_gap_bps,
            competing_quotes: f.competing_quotes,
            competing_low_minor: f.competing_low_minor,
            competing_high_minor: f.competing_high_minor,
            spread_bps: f.spread_bps,
            best_reference_milli: f.best_reference_milli,
            savings_opportunity_minor: line_savings,
            signals: line_signals.iter().map(|s| s.code().to_string()).collect(),
        });
    }
    signals.sort();
    Ok(Some(QuotationFacts {
        quotation_id: head.id,
        rfq_id: head.rfq_id,
        request_id: head.request_id,
        supplier_id: head.supplier_id,
        rfq_number: head.rfq_number,
        supplier_name: head.supplier_name,
        currency: currency.code().to_string(),
        superseded: head.superseded,
        quoted_total_minor: i64::try_from(total).map_err(|_| internal("total"))?,
        shipping_minor: head.shipping_minor,
        savings_opportunity_minor: i64::try_from(savings).unwrap_or(i64::MAX),
        thresholds: ThresholdsView {
            should_cost_bps: thresholds.should_cost_bps,
            history_bps: thresholds.history_bps,
        },
        signals: signals.iter().map(|s| s.code().to_string()).collect(),
        lines: out,
    }))
}

#[utoipa::path(
    operation_id = "cost_quotation_facts",
    get,
    path = "/api/v1/orgs/{org_id}/cost/quotations/{quotation_id}/facts",
    tag = "cost",
    params(("org_id" = Uuid, Path), ("quotation_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "The quotation against the should-cost, the lowest price paid and the other quotes", body = QuotationFacts),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown quotation", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn get_quotation_facts(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, quotation_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<QuotationFacts>> {
    require_read(&state, &p, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let facts = quotation_facts(&mut tx, quotation_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    tx.commit().await?;
    Ok(Json(facts))
}

// ---------------------------------------------------------------------------------------
// Negotiation briefs

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct BriefView {
    pub id: Uuid,
    pub request_id: Uuid,
    pub rfq_id: Uuid,
    pub supplier_id: Uuid,
    pub quotation_id: Uuid,
    pub exception_id: Option<Uuid>,
    pub currency: String,
    #[schema(value_type = QuotationFacts)]
    pub facts: Value,
    pub quoted_total_minor: i64,
    pub suggestion_id: Option<Uuid>,
    pub summary: String,
    #[schema(value_type = Vec<String>)]
    pub arguments: Value,
    pub target_price_minor: Option<i64>,
    /// `draft` | `final`
    pub status: String,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub finalised_by: Option<Uuid>,
    pub finalised_at: Option<DateTime<Utc>>,
}

const BRIEF_COLUMNS: &str = "id, request_id, rfq_id, supplier_id, quotation_id, exception_id, \
     currency, facts, quoted_total_minor, suggestion_id, summary, arguments, target_price_minor, \
     status, created_by, created_at, updated_at, finalised_by, finalised_at";

/// Saves the facts as the draft brief of their supplier and RFQ (a new draft, or the
/// existing one refreshed: a target above the new total and a narrative of another
/// quotation are cleared). Audits `negotiation_brief.saved`. Returns the brief id.
pub(crate) async fn save_brief(
    tx: &mut Tx,
    org_id: Uuid,
    p: &Principal,
    facts: &QuotationFacts,
    exception_id: Option<Uuid>,
) -> ApiResult<Uuid> {
    if facts.superseded {
        return Err(ApiError::Conflict("quotation_superseded"));
    }
    let stored = serde_json::to_value(facts).map_err(ApiError::internal)?;
    let (id, inserted): (Uuid, bool) = sqlx::query_as(
        "INSERT INTO negotiation_briefs (id, org_id, request_id, rfq_id, supplier_id, quotation_id,
                                         exception_id, currency, facts, quoted_total_minor,
                                         created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
         ON CONFLICT (rfq_id, supplier_id) WHERE status = 'draft' DO UPDATE SET
             suggestion_id = CASE WHEN negotiation_briefs.quotation_id = EXCLUDED.quotation_id
                                  THEN negotiation_briefs.suggestion_id END,
             target_price_minor = CASE WHEN negotiation_briefs.target_price_minor
                                            <= EXCLUDED.quoted_total_minor
                                       THEN negotiation_briefs.target_price_minor END,
             quotation_id = EXCLUDED.quotation_id, facts = EXCLUDED.facts,
             quoted_total_minor = EXCLUDED.quoted_total_minor,
             exception_id = coalesce(EXCLUDED.exception_id, negotiation_briefs.exception_id),
             updated_at = now()
         RETURNING id, (xmax = 0) AS inserted",
    )
    .bind(Uuid::now_v7())
    .bind(org_id)
    .bind(facts.request_id)
    .bind(facts.rfq_id)
    .bind(facts.supplier_id)
    .bind(facts.quotation_id)
    .bind(exception_id)
    .bind(&facts.currency)
    .bind(&stored)
    .bind(facts.quoted_total_minor)
    .bind(p.user_id)
    .fetch_one(&mut **tx)
    .await?;
    Audit { org_id, p }
        .record(
            tx,
            "negotiation_brief.saved",
            "negotiation_brief",
            id,
            json!({ "quotation": facts.quotation_id, "rfq": facts.rfq_number,
                    "supplier": facts.supplier_id, "exception": exception_id,
                    "created": inserted, "signals": facts.signals,
                    "savings_opportunity_minor": facts.savings_opportunity_minor }),
        )
        .await?;
    Ok(id)
}

async fn load_brief(tx: &mut Tx, id: Uuid, lock: bool) -> ApiResult<BriefView> {
    sqlx::query_as::<_, BriefView>(&format!(
        "SELECT {BRIEF_COLUMNS} FROM negotiation_briefs WHERE id = $1{}",
        if lock { " FOR UPDATE" } else { "" }
    ))
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateBriefInput {
    pub quotation_id: Uuid,
}

#[utoipa::path(
    operation_id = "cost_create_brief",
    post,
    path = "/api/v1/orgs/{org_id}/cost/briefs",
    tag = "cost",
    params(("org_id" = Uuid, Path)),
    request_body = CreateBriefInput,
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Draft brief with the quotation's facts (the supplier's existing draft is refreshed)", body = BriefView),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown quotation", body = ErrorBody),
        (status = 409, description = "The quotation was replaced by a later one", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn create_brief(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateBriefInput>,
) -> ApiResult<(StatusCode, Json<BriefView>)> {
    require_write(&state, &p, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let facts = quotation_facts(&mut tx, body.quotation_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let id = save_brief(&mut tx, org_id, &p, &facts, None).await?;
    let brief = load_brief(&mut tx, id, false).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(brief)))
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct BriefsQuery {
    pub rfq_id: Option<Uuid>,
    /// `draft` | `final`
    pub status: Option<String>,
}

#[utoipa::path(
    operation_id = "cost_list_briefs",
    get,
    path = "/api/v1/orgs/{org_id}/cost/briefs",
    tag = "cost",
    params(("org_id" = Uuid, Path), BriefsQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Briefs, newest first (at most 200)", body = [BriefView]),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown or unrelated organisation", body = ErrorBody),
        (status = 422, description = "Unknown status", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn list_briefs(
    State(state): State<AppState>,
    p: Principal,
    ApiPath(org_id): ApiPath<Uuid>,
    ApiQuery(q): ApiQuery<BriefsQuery>,
) -> ApiResult<Json<Vec<BriefView>>> {
    require_read(&state, &p, org_id).await?;
    if q.status
        .as_deref()
        .is_some_and(|s| !matches!(s, "draft" | "final"))
    {
        return Err(ApiError::Validation("status"));
    }
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let rows = sqlx::query_as::<_, BriefView>(&format!(
        "SELECT {BRIEF_COLUMNS} FROM negotiation_briefs
         WHERE ($1::uuid IS NULL OR rfq_id = $1) AND ($2::text IS NULL OR status = $2)
         ORDER BY created_at DESC, id DESC LIMIT 200"
    ))
    .bind(q.rfq_id)
    .bind(&q.status)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[utoipa::path(
    operation_id = "cost_get_brief",
    get,
    path = "/api/v1/orgs/{org_id}/cost/briefs/{brief_id}",
    tag = "cost",
    params(("org_id" = Uuid, Path), ("brief_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "One brief", body = BriefView),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown brief", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn get_brief(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, brief_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<Json<BriefView>> {
    require_read(&state, &p, org_id).await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let brief = load_brief(&mut tx, brief_id, false).await?;
    tx.commit().await?;
    Ok(Json(brief))
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct NarrativeView {
    pub brief: BriefView,
    /// `pending` (linked to the brief) or `failed` (with `error_code`).
    pub suggestion: SuggestionView,
}

#[utoipa::path(
    operation_id = "cost_draft_narrative",
    post,
    path = "/api/v1/orgs/{org_id}/cost/briefs/{brief_id}/narrative",
    tag = "cost",
    params(("org_id" = Uuid, Path), ("brief_id" = Uuid, Path)),
    security(("bearer" = [])),
    responses(
        (status = 201, description = "AI narrative drafted from the brief's facts only (a suggestion; a person finalises the brief)", body = NarrativeView),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown brief", body = ErrorBody),
        (status = 409, description = "Brief is final, or AI is off for the organisation", body = ErrorBody),
        (status = 429, description = "AI rate limit reached", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn draft_narrative(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, brief_id)): ApiPath<(Uuid, Uuid)>,
) -> ApiResult<(StatusCode, Json<NarrativeView>)> {
    require_write(&state, &p, org_id).await?;
    let brief = {
        let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
        let b = load_brief(&mut tx, brief_id, false).await?;
        tx.commit().await?;
        b
    };
    if brief.status != "draft" {
        return Err(ApiError::Conflict("brief_final"));
    }
    let facts: QuotationFacts =
        serde_json::from_value(brief.facts.clone()).map_err(|_| internal("stored facts"))?;
    ai::ai_allowed(&state, &p, org_id).await?;
    ai::rate_limit(&state, &p)?;
    let input = facts.narrative_input();
    let text: String = negotiation::facts_text(&input)
        .chars()
        .take(procure_ai::extract::MAX_INPUT_CHARS)
        .collect();
    let result = {
        let _permit = state
            .ai_permits
            .acquire()
            .await
            .map_err(ApiError::internal)?;
        negotiation::negotiation_brief(state.ai.as_ref(), &input, brief.quoted_total_minor).await
    };
    let suggestion = ai::store(
        &state,
        &p,
        NewSuggestion {
            org_id,
            capability: NEGOTIATION_BRIEF,
            subject_type: "rfq",
            subject_id: brief.rfq_id,
            supplier_id: Some(brief.supplier_id),
            text: &text,
            source: "typed",
        },
        &result,
    )
    .await?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    if suggestion.status == "pending" {
        let n = sqlx::query(
            "UPDATE negotiation_briefs SET suggestion_id = $2, updated_at = now()
             WHERE id = $1 AND status = 'draft' AND quotation_id = $3",
        )
        .bind(brief_id)
        .bind(suggestion.id)
        .bind(brief.quotation_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if n == 0 {
            return Err(ApiError::Conflict("brief_changed"));
        }
        Audit { org_id, p: &p }
            .record(
                &mut tx,
                "negotiation_brief.narrative_drafted",
                "negotiation_brief",
                brief_id,
                json!({ "suggestion": suggestion.id }),
            )
            .await?;
    }
    let brief = load_brief(&mut tx, brief_id, false).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(NarrativeView { brief, suggestion }),
    ))
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FinaliseBriefInput {
    pub summary: String,
    #[serde(default)]
    pub arguments: Vec<String>,
    /// For the quoted lines' total, minor units; never above it.
    pub target_price_minor: Option<i64>,
    /// The text was taken from the brief's AI narrative (marks it accepted, else rejected).
    #[serde(default)]
    pub used_suggestion: bool,
}

#[utoipa::path(
    operation_id = "cost_finalise_brief",
    post,
    path = "/api/v1/orgs/{org_id}/cost/briefs/{brief_id}/finalise",
    tag = "cost",
    params(("org_id" = Uuid, Path), ("brief_id" = Uuid, Path)),
    request_body = FinaliseBriefInput,
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Brief final (it never changes again)", body = BriefView),
        (status = 403, description = "Not permitted", body = ErrorBody),
        (status = 404, description = "Unknown brief", body = ErrorBody),
        (status = 409, description = "Already final", body = ErrorBody),
        (status = 422, description = "Invalid text or a target above the quoted total", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn finalise_brief(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, brief_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<FinaliseBriefInput>,
) -> ApiResult<Json<BriefView>> {
    require_write(&state, &p, org_id).await?;
    let summary = common::required(&body.summary, 4000, true, "summary")?;
    if body.arguments.len() > negotiation::MAX_ARGUMENTS {
        return Err(ApiError::Validation("arguments"));
    }
    let arguments = body
        .arguments
        .iter()
        .map(|a| common::required(a, negotiation::MAX_ARGUMENT_CHARS, true, "arguments"))
        .collect::<ApiResult<Vec<_>>>()?;
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    let brief = load_brief(&mut tx, brief_id, true).await?;
    if brief.status != "draft" {
        return Err(ApiError::Conflict("brief_final"));
    }
    if body
        .target_price_minor
        .is_some_and(|t| !(0..=brief.quoted_total_minor).contains(&t))
    {
        return Err(ApiError::Validation("target_price_minor"));
    }
    sqlx::query(
        "UPDATE negotiation_briefs SET status = 'final', summary = $2, arguments = $3,
                target_price_minor = $4, finalised_by = $5, finalised_at = now(),
                updated_at = now()
         WHERE id = $1",
    )
    .bind(brief_id)
    .bind(&summary)
    .bind(json!(arguments))
    .bind(body.target_price_minor)
    .bind(p.user_id)
    .execute(&mut *tx)
    .await?;
    if let Some(s) = brief.suggestion_id {
        sqlx::query(
            "UPDATE ai_suggestions SET status = $2, decided_by = $3, decided_at = now(),
                    decision_note = $4
             WHERE id = $1 AND status = 'pending'",
        )
        .bind(s)
        .bind(if body.used_suggestion {
            "accepted"
        } else {
            "rejected"
        })
        .bind(p.user_id)
        .bind("negotiation brief finalised")
        .execute(&mut *tx)
        .await?;
    }
    Audit { org_id, p: &p }
        .record(
            &mut tx,
            "negotiation_brief.finalised",
            "negotiation_brief",
            brief_id,
            json!({ "quotation": brief.quotation_id, "suggestion": brief.suggestion_id,
                    "used_suggestion": body.used_suggestion,
                    "target_price_minor": body.target_price_minor,
                    "arguments": arguments.len() }),
        )
        .await?;
    let brief = load_brief(&mut tx, brief_id, false).await?;
    tx.commit().await?;
    Ok(Json(brief))
}
