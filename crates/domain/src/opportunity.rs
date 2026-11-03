//! Where a factory can save, measured from its own purchase records: for each item, how much
//! it spent, how far its own prices spread, and how much it would have paid less had every
//! purchase been at the lowest price it already paid. That is a floor, not a forecast: it
//! uses only prices the factory itself got. Items bought often from one supplier were never
//! compared at all; they are listed by spend, with no amount claimed.

use std::collections::{BTreeMap, BTreeSet};

/// One purchase record (thousandths of a unit, and of a minor currency unit).
#[derive(Debug, Clone)]
pub struct Purchase {
    pub item_key: String,
    pub description: String,
    pub supplier: String,
    pub unit_price_milli: i64,
    pub quantity_milli: i64,
    pub currency: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opportunity {
    pub item_key: String,
    /// The most recent description seen for the item.
    pub description: String,
    pub currency: String,
    pub purchases: u32,
    pub suppliers: u32,
    pub spend_minor: i64,
    pub min_price_milli: i64,
    pub max_price_milli: i64,
    /// Σ (price − lowest price) × quantity, rounded down to the minor unit.
    pub potential_minor: i64,
    /// Bought more than once, always from the same supplier: never compared.
    pub single_supplier: bool,
}

const MILLI_SQ: i128 = 1_000_000;

fn clamp(v: i128) -> i64 {
    i64::try_from(v.max(0)).unwrap_or(i64::MAX)
}

/// Opportunities per item and currency, best first: largest potential, then largest spend,
/// then key. Records must be newest first, so the description is the latest one; the result
/// is the same for the same input.
pub fn assess(records: &[Purchase]) -> Vec<Opportunity> {
    let mut groups: BTreeMap<(&str, &str), Vec<&Purchase>> = BTreeMap::new();
    for r in records {
        if r.unit_price_milli < 0 || r.quantity_milli <= 0 {
            continue;
        }
        groups
            .entry((r.item_key.as_str(), r.currency.as_str()))
            .or_default()
            .push(r);
    }
    let mut out: Vec<Opportunity> = groups
        .into_iter()
        .filter_map(|((key, currency), rows)| {
            let min = rows.iter().map(|r| r.unit_price_milli).min()?;
            let max = rows.iter().map(|r| r.unit_price_milli).max()?;
            let spend: i128 = rows
                .iter()
                .map(|r| i128::from(r.unit_price_milli) * i128::from(r.quantity_milli))
                .sum();
            let over: i128 = rows
                .iter()
                .map(|r| i128::from(r.unit_price_milli - min) * i128::from(r.quantity_milli))
                .sum();
            let suppliers: BTreeSet<String> = rows
                .iter()
                .map(|r| {
                    r.supplier
                        .split_whitespace()
                        .collect::<String>()
                        .to_lowercase()
                })
                .collect();
            let n = u32::try_from(rows.len()).unwrap_or(u32::MAX);
            let s = u32::try_from(suppliers.len()).unwrap_or(u32::MAX);
            Some(Opportunity {
                item_key: key.to_string(),
                description: rows[0].description.clone(),
                currency: currency.to_string(),
                purchases: n,
                suppliers: s,
                spend_minor: clamp(spend / MILLI_SQ),
                min_price_milli: min,
                max_price_milli: max,
                potential_minor: clamp(over / MILLI_SQ),
                single_supplier: n > 1 && s == 1,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.potential_minor
            .cmp(&a.potential_minor)
            .then(b.spend_minor.cmp(&a.spend_minor))
            .then_with(|| a.item_key.cmp(&b.item_key))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(key: &str, supplier: &str, yen: i64, qty: i64) -> Purchase {
        Purchase {
            item_key: key.into(),
            description: format!("desc {key}"),
            supplier: supplier.into(),
            unit_price_milli: yen * 1000,
            quantity_milli: qty * 1000,
            currency: "JPY".into(),
        }
    }

    #[test]
    fn potential_is_what_was_paid_above_the_factorys_own_lowest_price() {
        let r = assess(&[
            p("m:A", "X", 1200, 10),
            p("m:A", "Y", 1000, 20),
            p("m:A", "X", 1100, 5),
            p("m:B", "Z", 50, 100),
            p("m:B", "Z", 50, 100),
        ]);
        assert_eq!(r.len(), 2);
        let a = &r[0];
        assert_eq!(a.item_key, "m:A");
        assert_eq!(a.spend_minor, 12_000 + 20_000 + 5_500);
        // (1200−1000)×10 + 0 + (1100−1000)×5
        assert_eq!(a.potential_minor, 2_500);
        assert_eq!((a.purchases, a.suppliers, a.single_supplier), (3, 2, false));
        let b = &r[1];
        assert_eq!((b.potential_minor, b.spend_minor), (0, 10_000));
        assert!(b.single_supplier);
    }

    #[test]
    fn fractions_round_down_and_currencies_are_never_mixed() {
        let mut usd = p("m:A", "X", 0, 3);
        usd.currency = "USD".into();
        usd.unit_price_milli = 1_234; // 1.234 cents
        let mut usd2 = usd.clone();
        usd2.unit_price_milli = 1_000;
        let r = assess(&[p("m:A", "X", 100, 1), usd, usd2]);
        assert_eq!(r.len(), 2);
        let u = r.iter().find(|o| o.currency == "USD").unwrap();
        // 0.234 × 3 = 0.702 cents → 0; spend 3.702 + 3 = 6.702 → 6.
        assert_eq!((u.potential_minor, u.spend_minor), (0, 6));
        // Same supplier written differently counts once.
        let r = assess(&[
            p("m:C", "Tokyo  Denki", 10, 1),
            p("m:C", "tokyo denki", 10, 1),
        ]);
        assert_eq!(r[0].suppliers, 1);
    }

    #[test]
    fn order_is_stable() {
        let rows = vec![
            p("m:B", "X", 10, 1),
            p("m:A", "X", 10, 1),
            p("m:C", "X", 20, 1),
        ];
        let keys: Vec<String> = assess(&rows).into_iter().map(|o| o.item_key).collect();
        assert_eq!(keys, vec!["m:C", "m:A", "m:B"]);
    }
}
