//! The decision memo next to a quotation comparison: the LAYA method (gather the evidence,
//! score every candidate, penalise risk, record why) applied to the customer's own data only.
//! Each compliant quotation is scored against the customer's written priorities (price, lead
//! time, existing supplier, domestic); risks (prices above the alert line, lead time beyond
//! the customer's limit) cost points; excluded suppliers and non-compliant quotations are
//! never recommended. Deterministic and explainable. It recommends; a person decides, and the
//! approval rules apply as before.

/// Points on the 0–1000 score.
const ALERT_PENALTY: u32 = 50;
const OVER_LEAD_PENALTY: u32 = 150;
/// Two best scores closer than this: say it is a close call.
const CLOSE_CALL: u32 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Factor {
    Price,
    LeadTime,
    ExistingSupplier,
    Domestic,
}

impl Factor {
    pub const fn code(self) -> &'static str {
        match self {
            Factor::Price => "price",
            Factor::LeadTime => "lead_time",
            Factor::ExistingSupplier => "existing_supplier",
            Factor::Domestic => "domestic",
        }
    }
}

/// What is known about one quotation.
#[derive(Debug, Clone, Default)]
pub struct Candidate {
    pub compliant: bool,
    /// In the request currency; `None` when it cannot be compared without FX.
    pub total_minor: Option<i64>,
    pub max_lead_time_days: Option<u32>,
    /// Supplier based in Japan; `None` unknown.
    pub domestic: Option<bool>,
    pub preferred: bool,
    pub excluded: bool,
    /// Past purchases from this supplier in the customer's history.
    pub past_purchases: u32,
    /// Lines priced above the customer's alert line (vs their last purchase).
    pub price_alerts: u32,
}

/// The customer's buying criteria, as far as the memo uses them.
#[derive(Debug, Clone, Default)]
pub struct Criteria {
    /// Priority codes, most important first (`price`, `lead_time`, `quality`,
    /// `existing_supplier`, `domestic`).
    pub priorities: Vec<String>,
    pub max_lead_time_days: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactorScore {
    pub factor: Factor,
    pub weight: u32,
    /// 0–100.
    pub points: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Assessment {
    /// 0–1000; `None` when the quotation cannot be recommended at all.
    pub score: Option<u32>,
    /// 1 = recommended; only scored quotations are ranked.
    pub rank: Option<u32>,
    pub factors: Vec<FactorScore>,
    /// Why points were lost or why it is not ranked, and what a person should check.
    pub cautions: Vec<&'static str>,
}

/// Weight of each factor: by position in the customer's priorities (10, 4, 2, 1, 1), so the
/// first priority clearly leads; price and lead time count 1 even when not listed; without
/// any priorities, price 3, lead time 1.
fn weights(c: &Criteria) -> Vec<(Factor, u32)> {
    let order: Vec<&str> = c.priorities.iter().map(String::as_str).collect();
    let by_position = [10, 4, 2, 1, 1];
    let weight = |code: &str, default: u32| {
        order
            .iter()
            .position(|p| *p == code)
            .map_or(default, |i| by_position.get(i).copied().unwrap_or(1))
    };
    let (price_default, lead_default) = if order.is_empty() { (3, 1) } else { (1, 1) };
    [
        (Factor::Price, weight("price", price_default)),
        (Factor::LeadTime, weight("lead_time", lead_default)),
        (Factor::ExistingSupplier, weight("existing_supplier", 0)),
        (Factor::Domestic, weight("domestic", 0)),
    ]
    .into_iter()
    .filter(|(_, w)| *w > 0)
    .collect()
}

/// 100 for the best value, 2 points less for every percent above it (0 at +50 %). `floor`
/// keeps tiny bests from exaggerating (1 day vs 3 days is not +200 %).
fn points(best: u64, this: u64, floor: u64) -> u32 {
    let base = best.max(floor).max(1);
    let over_pct = this.saturating_sub(best) * 100 / base;
    u32::try_from(100u64.saturating_sub(over_pct * 2)).unwrap_or(0)
}

pub fn assess(candidates: &[Candidate], criteria: &Criteria) -> Vec<Assessment> {
    let eligible = |c: &Candidate| c.compliant && !c.excluded && c.total_minor.is_some();
    let best_total = candidates
        .iter()
        .filter(|c| eligible(c))
        .filter_map(|c| c.total_minor)
        .map(|t| u64::try_from(t).unwrap_or(0))
        .min();
    let best_lead = candidates
        .iter()
        .filter(|c| eligible(c))
        .filter_map(|c| c.max_lead_time_days)
        .min();
    let weights = weights(criteria);
    let quality_asked = criteria.priorities.iter().any(|p| p == "quality");

    let mut out: Vec<Assessment> = candidates
        .iter()
        .map(|c| {
            let mut a = Assessment::default();
            if c.excluded {
                a.cautions.push("excluded_supplier");
            }
            if !c.compliant {
                a.cautions.push("not_compliant");
            }
            if c.total_minor.is_none() {
                a.cautions.push("total_not_comparable");
            }
            if !eligible(c) {
                return a;
            }
            for (factor, weight) in &weights {
                let points = match factor {
                    Factor::Price => points(
                        best_total.unwrap_or(0),
                        u64::try_from(c.total_minor.unwrap_or(0)).unwrap_or(0),
                        1,
                    ),
                    Factor::LeadTime => match (best_lead, c.max_lead_time_days) {
                        (Some(b), Some(l)) => points(u64::from(b), u64::from(l), 7),
                        _ => {
                            a.cautions.push("lead_time_unknown");
                            0
                        }
                    },
                    Factor::ExistingSupplier => {
                        if c.preferred {
                            100
                        } else if c.past_purchases > 0 {
                            70
                        } else {
                            0
                        }
                    }
                    Factor::Domestic => match c.domestic {
                        Some(true) => 100,
                        Some(false) => 0,
                        None => {
                            a.cautions.push("origin_unknown");
                            0
                        }
                    },
                };
                a.factors.push(FactorScore {
                    factor: *factor,
                    weight: *weight,
                    points,
                });
            }
            let total_weight: u32 = a.factors.iter().map(|f| f.weight).sum();
            let weighted: u32 = a.factors.iter().map(|f| f.weight * f.points).sum();
            let mut score = (weighted * 10).checked_div(total_weight).unwrap_or(0);
            if c.price_alerts > 0 {
                a.cautions.push("price_above_alert");
                score = score.saturating_sub(ALERT_PENALTY * c.price_alerts);
            }
            if let (Some(max), Some(lead)) = (criteria.max_lead_time_days, c.max_lead_time_days)
                && lead > max
            {
                a.cautions.push("over_max_lead_time");
                score = score.saturating_sub(OVER_LEAD_PENALTY);
            }
            if quality_asked {
                // No quality record is kept yet: say so rather than score it.
                a.cautions.push("quality_not_assessed");
            }
            a.score = Some(score);
            a
        })
        .collect();

    // Rank: score, then the lower total, then input order (stable).
    let mut order: Vec<usize> = (0..out.len()).filter(|i| out[*i].score.is_some()).collect();
    order.sort_by(|a, b| {
        out[*b]
            .score
            .cmp(&out[*a].score)
            .then_with(|| candidates[*a].total_minor.cmp(&candidates[*b].total_minor))
    });
    for (rank, i) in order.iter().enumerate() {
        out[*i].rank = u32::try_from(rank + 1).ok();
    }
    if let [first, second, ..] = order.as_slice() {
        let (s1, s2) = (
            out[*first].score.unwrap_or(0),
            out[*second].score.unwrap_or(0),
        );
        if s1 - s2.min(s1) < CLOSE_CALL {
            out[*first].cautions.push("close_call");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quote(total: i64, lead: u32) -> Candidate {
        Candidate {
            compliant: true,
            total_minor: Some(total),
            max_lead_time_days: Some(lead),
            domestic: Some(true),
            ..Candidate::default()
        }
    }

    fn criteria(p: &[&str], max_lead: Option<u32>) -> Criteria {
        Criteria {
            priorities: p.iter().map(|s| (*s).to_string()).collect(),
            max_lead_time_days: max_lead,
        }
    }

    #[test]
    fn the_customers_first_priority_decides() {
        // Cheaper but 4 days slower, vs 20 % dearer and faster.
        let q = [quote(10_000, 14), quote(12_000, 10)];
        let by_price = assess(&q, &criteria(&["price", "lead_time"], None));
        // price 100×10 + lead (40 % slower → 20)×4 = 1080 / 14; price 60×10 + lead 100×4.
        assert_eq!(by_price[0].score, Some(771));
        assert_eq!(by_price[1].score, Some(714));
        assert_eq!(by_price[0].rank, Some(1));
        let by_lead = assess(&q, &criteria(&["lead_time", "price"], None));
        assert_eq!(by_lead[1].rank, Some(1));
        assert_eq!(by_lead[1].score, Some(885));
    }

    #[test]
    fn excluded_or_non_compliant_quotes_are_never_recommended() {
        let mut excluded = quote(5_000, 1);
        excluded.excluded = true;
        let mut broken = quote(4_000, 1);
        broken.compliant = false;
        let a = assess(
            &[excluded, broken, quote(9_000, 10)],
            &criteria(&["price"], None),
        );
        assert_eq!(a[0].score, None);
        assert_eq!(a[0].cautions, vec!["excluded_supplier"]);
        assert_eq!(a[1].cautions, vec!["not_compliant"]);
        assert_eq!(a[2].rank, Some(1));
        assert_eq!(a[2].score, Some(1000));
    }

    #[test]
    fn risks_cost_points_and_are_explained() {
        let mut alerted = quote(10_000, 5);
        alerted.price_alerts = 2;
        let slow = quote(10_000, 40);
        let a = assess(&[alerted, slow], &criteria(&["price", "quality"], Some(30)));
        assert!(a[0].cautions.contains(&"price_above_alert"));
        assert!(a[1].cautions.contains(&"over_max_lead_time"));
        assert!(
            a.iter()
                .all(|x| x.cautions.contains(&"quality_not_assessed"))
        );
        // price 100×10 + lead 100×1 → 1000 − 2×50; price 100×10 + lead 0×1 → 909 − 150.
        assert_eq!(a[0].score, Some(900));
        assert_eq!(a[1].score, Some(759));
    }

    #[test]
    fn existing_suppliers_and_domestic_count_when_the_customer_asks() {
        let mut known = quote(11_000, 10);
        known.past_purchases = 4;
        let mut new_abroad = quote(10_000, 10);
        new_abroad.domestic = Some(false);
        let c = criteria(&["existing_supplier", "domestic", "price"], None);
        let a = assess(&[known, new_abroad], &c);
        assert_eq!(a[0].rank, Some(1));
        let codes: Vec<&str> = a[0].factors.iter().map(|f| f.factor.code()).collect();
        assert_eq!(
            codes,
            vec!["price", "lead_time", "existing_supplier", "domestic"]
        );
    }

    #[test]
    fn close_calls_are_flagged_and_ties_go_to_the_lower_total() {
        let a = assess(
            &[quote(10_100, 10), quote(10_000, 10)],
            &criteria(&[], None),
        );
        assert_eq!(a[1].rank, Some(1));
        assert!(a[1].cautions.contains(&"close_call"));
        assert!(assess(&[], &criteria(&[], None)).is_empty());
    }
}
