//! The suppliers to ask for a request: verified directory suppliers the factory has not
//! rejected, ranked by `procure_domain::shortlist` from this factory's own record only, with
//! about ten pre-selected. Nothing is sent or stored: staff create the RFQ from the list,
//! then check it before it goes out.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use axum::Json;
use axum::extract::State;
use procure_domain::history;
use procure_domain::rbac::Permission;
use procure_domain::shortlist::{self, Approval, Candidate, Reason};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;
use crate::api::history::key_groups;
use crate::api::rfqs::{require_operator, supplier_name_key};
use crate::db;
use crate::error::{ApiError, ApiPath, ApiQuery, ApiResult, ErrorBody};
use crate::principal::Principal;

#[derive(Deserialize, utoipa::IntoParams)]
pub struct ShortlistQuery {
    /// How many to pre-select (1–20, default 10).
    pub picks: Option<usize>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ShortlistReason {
    /// quoted_before, cheapest_before, ordered_before, bought_before, category,
    /// approved_by_factory, replies, no_replies, already_invited.
    pub code: &'static str,
    pub count: Option<u32>,
    /// For replies: out of how many requests.
    pub of: Option<u32>,
    pub category: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ShortlistPick {
    pub supplier_id: Uuid,
    pub name: String,
    pub country: String,
    pub score: i64,
    pub selected: bool,
    /// Something says it sells these items (not only a good record).
    pub relevant: bool,
    pub reasons: Vec<ShortlistReason>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ShortlistView {
    pub picks: Vec<ShortlistPick>,
}

fn reason(r: Reason) -> ShortlistReason {
    let (code, count, of, category) = match r {
        Reason::QuotedBefore { items } => ("quoted_before", Some(items), None, None),
        Reason::CheapestBefore { items } => ("cheapest_before", Some(items), None, None),
        Reason::OrderedBefore { orders } => ("ordered_before", Some(orders), None, None),
        Reason::BoughtBefore => ("bought_before", None, None, None),
        Reason::Category { category } => ("category", None, None, Some(category)),
        Reason::ApprovedByFactory => ("approved_by_factory", None, None, None),
        Reason::Replies { responded, invited } => ("replies", Some(responded), Some(invited), None),
        Reason::NoReplies { invited } => ("no_replies", None, Some(invited), None),
        Reason::AlreadyInvited => ("already_invited", None, None, None),
    };
    ShortlistReason {
        code,
        count,
        of,
        category,
    }
}

#[derive(sqlx::FromRow)]
struct ItemRow {
    description: String,
    manufacturer: String,
    mpn: String,
    verified_mpn: Option<String>,
    notes: String,
}

#[derive(sqlx::FromRow)]
struct PastLine {
    supplier_id: Uuid,
    mpn: String,
    verified_mpn: Option<String>,
    description: String,
    unit_price_minor: i64,
    currency: String,
}

#[derive(sqlx::FromRow)]
struct SupplierRow {
    id: Uuid,
    name: String,
    country: String,
    categories: Vec<String>,
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_id}/requests/{request_id}/supplier-shortlist",
    tag = "rfqs",
    params(("org_id" = Uuid, Path), ("request_id" = Uuid, Path), ShortlistQuery),
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Ranked suppliers, with the pre-selected ones marked", body = ShortlistView),
        (status = 403, description = "Not assigned staff", body = ErrorBody),
        (status = 404, description = "Unknown request or not assigned", body = ErrorBody),
        (status = 422, description = "picks out of range", body = ErrorBody),
        (status = 400, description = "Malformed request", body = ErrorBody),
        (status = 401, description = "No valid session", body = ErrorBody),
    )
)]
pub async fn supplier_shortlist(
    State(state): State<AppState>,
    p: Principal,
    ApiPath((org_id, request_id)): ApiPath<(Uuid, Uuid)>,
    ApiQuery(q): ApiQuery<ShortlistQuery>,
) -> ApiResult<Json<ShortlistView>> {
    require_operator(&p, &state, org_id, Permission::ManageRfqs).await?;
    let picks = q.picks.unwrap_or(shortlist::DEFAULT_PICKS);
    if !(1..=shortlist::MAX_PICKS).contains(&picks) {
        return Err(ApiError::Validation("picks"));
    }
    let mut tx = db::tenant_tx(&state.pool, p.user_id, org_id).await?;
    db::enable_staff_scope(&mut tx).await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM procurement_requests WHERE id = $1)")
            .bind(request_id)
            .fetch_one(&mut *tx)
            .await?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    let items = sqlx::query_as::<_, ItemRow>(
        "SELECT description, manufacturer, mpn, verified_mpn, notes FROM request_items
         WHERE request_id = $1 ORDER BY line_no",
    )
    .bind(request_id)
    .fetch_all(&mut *tx)
    .await?;
    let lines: Vec<String> = items
        .iter()
        .map(|i| {
            format!(
                "{} {} {} {} {}",
                i.description,
                i.manufacturer,
                i.mpn,
                i.verified_mpn.as_deref().unwrap_or(""),
                i.notes
            )
        })
        .collect();
    // Each key of an item in this request (aliases included) → the item's index.
    let own: Vec<(usize, String)> = items
        .iter()
        .enumerate()
        .filter_map(|(n, i)| {
            history::item_key(i.verified_mpn.as_deref().unwrap_or(&i.mpn), &i.description)
                .map(|k| (n, k))
        })
        .collect();
    let keys: Vec<String> = own.iter().map(|(_, k)| k.clone()).collect();
    let groups = key_groups(&mut tx, &keys).await?;
    let mut item_of: HashMap<String, usize> = HashMap::new();
    for (n, k) in &own {
        for alias in groups.get(k).into_iter().flatten() {
            item_of.entry(alias.clone()).or_insert(*n);
        }
    }

    // Current quotation lines this factory received for other requests (newest first).
    let past = sqlx::query_as::<_, PastLine>(
        "SELECT rr.supplier_id, ri.mpn, ri.verified_mpn, ri.description, ql.unit_price_minor,
                q.currency
         FROM quotation_lines ql
         JOIN quotations q ON q.id = ql.quotation_id AND q.superseded_at IS NULL
         JOIN rfq_recipients rr ON rr.id = q.rfq_recipient_id
         JOIN request_items ri ON ri.id = ql.request_item_id
         WHERE ri.request_id <> $1 AND NOT ql.alternative
         ORDER BY q.received_at DESC LIMIT 20000",
    )
    .bind(request_id)
    .fetch_all(&mut *tx)
    .await?;
    // Per (item, currency): each supplier's lowest price.
    let mut quoted: BTreeMap<(usize, String), BTreeMap<Uuid, i64>> = BTreeMap::new();
    for l in &past {
        let Some(n) =
            history::item_key(l.verified_mpn.as_deref().unwrap_or(&l.mpn), &l.description)
                .and_then(|k| item_of.get(&k).copied())
        else {
            continue;
        };
        let best = quoted
            .entry((n, l.currency.clone()))
            .or_default()
            .entry(l.supplier_id)
            .or_insert(l.unit_price_minor);
        *best = (*best).min(l.unit_price_minor);
    }
    let mut quoted_items: HashMap<Uuid, BTreeSet<usize>> = HashMap::new();
    let mut cheapest_items: HashMap<Uuid, BTreeSet<usize>> = HashMap::new();
    for ((n, _), by_supplier) in &quoted {
        let low = by_supplier.values().min().copied();
        for (s, price) in by_supplier {
            quoted_items.entry(*s).or_default().insert(*n);
            if Some(*price) == low && by_supplier.len() > 1 {
                cheapest_items.entry(*s).or_default().insert(*n);
            }
        }
    }

    let orders: HashMap<Uuid, i64> =
        sqlx::query_as("SELECT supplier_id, count(*) FROM purchase_orders GROUP BY supplier_id")
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .collect();
    // Earlier requests count once they had time to be answered: answered, or sent more than
    // two weeks ago and past their reply date.
    let replies: HashMap<Uuid, (i64, i64)> = sqlx::query_as::<_, (Uuid, i64, i64)>(
        "SELECT rr.supplier_id,
                count(*),
                count(*) FILTER (WHERE rr.status = 'responded')
         FROM rfq_recipients rr JOIN rfqs r ON r.id = rr.rfq_id
         WHERE r.status <> 'draft' AND r.request_id <> $1
           AND (rr.status = 'responded'
                OR (r.sent_at < now() - interval '14 days'
                    AND (r.response_due IS NULL OR r.response_due < current_date)))
         GROUP BY rr.supplier_id",
    )
    .bind(request_id)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|(s, a, b)| (s, (a, b)))
    .collect();
    let invited_here: BTreeSet<Uuid> = sqlx::query_scalar(
        "SELECT rr.supplier_id FROM rfq_recipients rr JOIN rfqs r ON r.id = rr.rfq_id
         WHERE r.request_id = $1",
    )
    .bind(request_id)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .collect();
    let bought: BTreeSet<String> = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT supplier_name FROM purchase_history WHERE supplier_name <> '' LIMIT 5000",
    )
    .fetch_all(&mut *tx)
    .await?
    .iter()
    .map(|n| supplier_name_key(n))
    .collect();
    let approvals: HashMap<Uuid, String> = sqlx::query_as(
        "SELECT supplier_id, status FROM customer_supplier_approvals WHERE org_id = $1",
    )
    .bind(org_id)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .collect();
    let suppliers = sqlx::query_as::<_, SupplierRow>(
        "SELECT id, name, country, categories FROM suppliers
         WHERE status = 'verified'
           AND id NOT IN (SELECT supplier_id FROM customer_supplier_approvals
                          WHERE org_id = $1 AND status = 'rejected')
         ORDER BY name, id LIMIT 500",
    )
    .bind(org_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let country: HashMap<Uuid, String> = suppliers
        .iter()
        .map(|s| (s.id, s.country.clone()))
        .collect();
    let candidates: Vec<Candidate<Uuid>> = suppliers
        .into_iter()
        .map(|s| {
            let (invited, responded) = replies.get(&s.id).copied().unwrap_or((0, 0));
            Candidate {
                id: s.id,
                quoted_items: count(quoted_items.get(&s.id).map_or(0, BTreeSet::len)),
                cheapest_items: count(cheapest_items.get(&s.id).map_or(0, BTreeSet::len)),
                orders: u32::try_from(orders.get(&s.id).copied().unwrap_or(0)).unwrap_or(u32::MAX),
                bought_before: bought.contains(&supplier_name_key(&s.name)),
                approval: match approvals.get(&s.id).map(String::as_str) {
                    Some("approved") => Approval::Approved,
                    Some("proposed") => Approval::Proposed,
                    _ => Approval::None,
                },
                invited: u32::try_from(invited).unwrap_or(u32::MAX),
                responded: u32::try_from(responded).unwrap_or(u32::MAX),
                already_invited: invited_here.contains(&s.id),
                name: s.name,
                categories: s.categories,
            }
        })
        .collect();
    let ranked = shortlist::rank(&candidates, &lines, count(items.len()), picks);
    Ok(Json(ShortlistView {
        picks: ranked
            .into_iter()
            .map(|r| ShortlistPick {
                supplier_id: r.id,
                country: country.get(&r.id).cloned().unwrap_or_default(),
                name: r.name,
                score: r.score,
                selected: r.selected,
                relevant: r.relevant,
                reasons: r.reasons.into_iter().map(reason).collect(),
            })
            .collect(),
    }))
}
