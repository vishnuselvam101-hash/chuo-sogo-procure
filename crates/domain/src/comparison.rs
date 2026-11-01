//! Deterministic quotation comparison: compliance checks, totals in integer minor units,
//! and a stable ranking. Same input always gives the same output; no FX conversion and
//! no AI involvement. A human operator reviews the result before recommending.

use crate::money::{Currency, Money};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Compliance {
    Compliant,
    NeedsReview,
    Incomplete,
    NonCompliant,
}

impl Compliance {
    pub const fn code(self) -> &'static str {
        match self {
            Compliance::Compliant => "compliant",
            Compliance::NeedsReview => "needs_review",
            Compliance::Incomplete => "incomplete",
            Compliance::NonCompliant => "non_compliant",
        }
    }
}

/// Machine-readable reasons; labels live in the client's locale files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reason {
    MissingItem,
    InsufficientQuantity,
    Expired,
    CurrencyDiffers,
    AlternativeOffered,
    DuplicateLine,
    UnknownItem,
    AmountOverflow,
}

impl Reason {
    pub const fn code(self) -> &'static str {
        match self {
            Reason::MissingItem => "missing_item",
            Reason::InsufficientQuantity => "insufficient_quantity",
            Reason::Expired => "expired",
            Reason::CurrencyDiffers => "currency_differs",
            Reason::AlternativeOffered => "alternative_offered",
            Reason::DuplicateLine => "duplicate_line",
            Reason::UnknownItem => "unknown_item",
            Reason::AmountOverflow => "amount_overflow",
        }
    }

    const fn severity(self) -> Compliance {
        match self {
            Reason::Expired | Reason::InsufficientQuantity | Reason::UnknownItem => {
                Compliance::NonCompliant
            }
            Reason::MissingItem => Compliance::Incomplete,
            Reason::CurrencyDiffers
            | Reason::AlternativeOffered
            | Reason::DuplicateLine
            | Reason::AmountOverflow => Compliance::NeedsReview,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RequiredItem<Id> {
    pub id: Id,
    pub quantity: u32,
}

#[derive(Debug, Clone)]
pub struct Line<Id> {
    pub item: Id,
    pub unit_price: Money,
    pub quantity: u32,
    pub lead_time_days: Option<u32>,
    /// The supplier offers a different product than the verified one.
    pub alternative: bool,
}

#[derive(Debug, Clone)]
pub struct Quote<Id> {
    pub id: Id,
    pub shipping: Money,
    /// Computed by the caller against the comparison date.
    pub expired: bool,
    pub lines: Vec<Line<Id>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation<Id> {
    pub quote: Id,
    pub compliance: Compliance,
    /// Sorted, deduplicated.
    pub reasons: Vec<Reason>,
    /// Lines + shipping; `None` when it cannot be computed without FX or overflowed.
    pub total: Option<Money>,
    /// Slowest line; `None` if no line states a lead time.
    pub max_lead_time_days: Option<u32>,
    /// 1-based among compliant quotations only.
    pub rank: Option<u32>,
}

fn evaluate<Id: Ord + Clone>(
    currency: Currency,
    items: &[RequiredItem<Id>],
    q: &Quote<Id>,
) -> Evaluation<Id> {
    let mut reasons = Vec::new();
    if q.expired {
        reasons.push(Reason::Expired);
    }
    for item in items {
        let lines: Vec<_> = q.lines.iter().filter(|l| l.item == item.id).collect();
        match lines.as_slice() {
            [] => reasons.push(Reason::MissingItem),
            [line] => {
                if line.quantity < item.quantity {
                    reasons.push(Reason::InsufficientQuantity);
                }
                if line.alternative {
                    reasons.push(Reason::AlternativeOffered);
                }
            }
            _ => reasons.push(Reason::DuplicateLine),
        }
    }
    if q.lines
        .iter()
        .any(|l| !items.iter().any(|i| i.id == l.item))
    {
        reasons.push(Reason::UnknownItem);
    }
    let same_currency = q.shipping.currency() == currency
        && q.lines.iter().all(|l| l.unit_price.currency() == currency);
    let total = if same_currency {
        let amounts: Result<Vec<Money>, _> = q
            .lines
            .iter()
            .map(|l| l.unit_price.checked_mul_qty(l.quantity))
            .collect();
        match amounts.and_then(|a| Money::sum(currency, &a)?.checked_add(q.shipping)) {
            Ok(t) => Some(t),
            Err(_) => {
                reasons.push(Reason::AmountOverflow);
                None
            }
        }
    } else {
        reasons.push(Reason::CurrencyDiffers);
        None
    };
    reasons.sort();
    reasons.dedup();
    let compliance = reasons
        .iter()
        .map(|r| r.severity())
        .max()
        .unwrap_or(Compliance::Compliant);
    Evaluation {
        quote: q.id.clone(),
        compliance,
        reasons,
        total,
        max_lead_time_days: q.lines.iter().filter_map(|l| l.lead_time_days).max(),
        rank: None,
    }
}

/// Evaluates every quotation. Compliant ones come first, ranked by total, then slowest
/// lead time (unknown counts as slowest), then quotation id; the rest follow by severity
/// and id, unranked.
pub fn compare<Id: Ord + Clone>(
    currency: Currency,
    items: &[RequiredItem<Id>],
    quotes: &[Quote<Id>],
) -> Vec<Evaluation<Id>> {
    let mut evals: Vec<_> = quotes
        .iter()
        .map(|q| evaluate(currency, items, q))
        .collect();
    evals.sort_by(|a, b| {
        a.compliance.cmp(&b.compliance).then_with(|| {
            if a.compliance == Compliance::Compliant {
                let key = |e: &Evaluation<Id>| {
                    (
                        e.total.map_or(i64::MAX, Money::minor),
                        e.max_lead_time_days.unwrap_or(u32::MAX),
                    )
                };
                key(a).cmp(&key(b)).then_with(|| a.quote.cmp(&b.quote))
            } else {
                a.quote.cmp(&b.quote)
            }
        })
    });
    let mut rank = 0;
    for e in &mut evals {
        if e.compliance == Compliance::Compliant {
            rank += 1;
            e.rank = Some(rank);
        }
    }
    evals
}

#[cfg(test)]
mod tests {
    use super::*;

    const JPY: Currency = Currency::Jpy;

    fn yen(n: i64) -> Money {
        Money::from_minor(n, JPY)
    }

    fn items() -> Vec<RequiredItem<u32>> {
        vec![
            RequiredItem {
                id: 1,
                quantity: 10,
            },
            RequiredItem { id: 2, quantity: 2 },
        ]
    }

    fn line(item: u32, price: i64, qty: u32, lead: Option<u32>) -> Line<u32> {
        Line {
            item,
            unit_price: yen(price),
            quantity: qty,
            lead_time_days: lead,
            alternative: false,
        }
    }

    fn quote(id: u32, shipping: i64, lines: Vec<Line<u32>>) -> Quote<u32> {
        Quote {
            id,
            shipping: yen(shipping),
            expired: false,
            lines,
        }
    }

    #[test]
    fn totals_are_exact_integer_sums() {
        let q = quote(
            7,
            1_500,
            vec![line(1, 1_234, 10, Some(5)), line(2, 50_000, 2, Some(14))],
        );
        let r = compare(JPY, &items(), &[q]);
        assert_eq!(r[0].total, Some(yen(12_340 + 100_000 + 1_500)));
        assert_eq!(r[0].compliance, Compliance::Compliant);
        assert_eq!(r[0].max_lead_time_days, Some(14));
        assert_eq!(r[0].rank, Some(1));
    }

    #[test]
    fn ranking_is_by_total_then_lead_time_then_id() {
        let full = |id, ship, lead| {
            quote(
                id,
                ship,
                vec![line(1, 100, 10, Some(lead)), line(2, 100, 2, Some(lead))],
            )
        };
        let quotes = [full(3, 0, 9), full(1, 0, 9), full(2, 0, 3), full(4, 10, 1)];
        let r = compare(JPY, &items(), &quotes);
        let order: Vec<_> = r.iter().map(|e| (e.quote, e.rank)).collect();
        assert_eq!(
            order,
            vec![(2, Some(1)), (1, Some(2)), (3, Some(3)), (4, Some(4))]
        );
        // Input order never changes the result.
        let mut reversed = quotes.to_vec();
        reversed.reverse();
        assert_eq!(compare(JPY, &items(), &reversed), r);
    }

    #[test]
    fn non_compliant_quotes_are_never_ranked() {
        let cheap_but_short = quote(1, 0, vec![line(1, 1, 9, None), line(2, 1, 2, None)]);
        let missing = quote(2, 0, vec![line(1, 1, 10, None)]);
        let mut expired = quote(3, 0, vec![line(1, 1, 10, None), line(2, 1, 2, None)]);
        expired.expired = true;
        let ok = quote(4, 0, vec![line(1, 999, 10, None), line(2, 999, 2, None)]);
        let r = compare(JPY, &items(), &[cheap_but_short, missing, expired, ok]);
        assert_eq!(r[0].quote, 4);
        assert_eq!(r[0].rank, Some(1));
        for e in &r[1..] {
            assert_eq!(e.rank, None);
        }
        let by_id = |id| r.iter().find(|e| e.quote == id).unwrap();
        assert_eq!(by_id(1).reasons, vec![Reason::InsufficientQuantity]);
        assert_eq!(by_id(1).compliance, Compliance::NonCompliant);
        assert_eq!(by_id(2).compliance, Compliance::Incomplete);
        assert_eq!(by_id(3).reasons, vec![Reason::Expired]);
    }

    #[test]
    fn foreign_currency_needs_review_and_has_no_total() {
        let mut q = quote(1, 0, vec![line(1, 1, 10, None), line(2, 1, 2, None)]);
        q.lines[0].unit_price = Money::from_minor(100, Currency::Usd);
        let r = compare(JPY, &items(), &[q]);
        assert_eq!(r[0].compliance, Compliance::NeedsReview);
        assert_eq!(r[0].reasons, vec![Reason::CurrencyDiffers]);
        assert_eq!(r[0].total, None);
    }

    #[test]
    fn alternatives_and_duplicates_need_review() {
        let mut alt = quote(1, 0, vec![line(1, 1, 10, None), line(2, 1, 2, None)]);
        alt.lines[1].alternative = true;
        let dup = quote(
            2,
            0,
            vec![
                line(1, 1, 10, None),
                line(1, 1, 10, None),
                line(2, 1, 2, None),
            ],
        );
        let r = compare(JPY, &items(), &[alt, dup]);
        assert_eq!(r[0].reasons, vec![Reason::AlternativeOffered]);
        assert_eq!(r[1].reasons, vec![Reason::DuplicateLine]);
        assert!(r.iter().all(|e| e.compliance == Compliance::NeedsReview));
    }

    #[test]
    fn unknown_items_are_non_compliant() {
        let q = quote(
            1,
            0,
            vec![
                line(1, 1, 10, None),
                line(2, 1, 2, None),
                line(99, 1, 1, None),
            ],
        );
        let r = compare(JPY, &items(), &[q]);
        assert_eq!(r[0].compliance, Compliance::NonCompliant);
        assert_eq!(r[0].reasons, vec![Reason::UnknownItem]);
    }

    #[test]
    fn overflow_is_reported_not_wrapped() {
        let q = quote(
            1,
            0,
            vec![line(1, i64::MAX / 2, 10, None), line(2, 1, 2, None)],
        );
        let r = compare(JPY, &items(), &[q]);
        assert_eq!(r[0].total, None);
        assert_eq!(r[0].reasons, vec![Reason::AmountOverflow]);
        assert_eq!(r[0].rank, None);
    }
}
