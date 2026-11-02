//! Savings share (the factory-side fee model). We charge no monthly fee: for each order we
//! place for the factory, the saving is (price paid before − price now) × quantity, and our
//! fee is an agreed share of that saving (50% by default) for an agreed term (36 months by
//! default). No saving, no fee — a price at or above the baseline gives 0, never a negative.
//!
//! All amounts are exact integers. Baselines come from purchase history in thousandths of a
//! minor unit (`unit_price_milli`), so sub-yen unit prices are not lost; the money results
//! are rounded down to whole minor units, in the factory's favour.

use crate::history::Day;
use crate::money::{Currency, Money};

pub const DEFAULT_SHARE_BPS: u32 = 5_000;
pub const DEFAULT_TERM_MONTHS: u32 = 36;
pub const MAX_TERM_MONTHS: u32 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavingsError {
    CurrencyMismatch,
    UnitMismatch,
    Overflow,
    InvalidShare,
    InvalidTerm,
}

impl std::fmt::Display for SavingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SavingsError::CurrencyMismatch => "baseline and order are in different currencies",
            SavingsError::UnitMismatch => "baseline and order use different units",
            SavingsError::Overflow => "savings amount overflow",
            SavingsError::InvalidShare => "share must be between 1 and 10000 basis points",
            SavingsError::InvalidTerm => "term must be between 1 and 120 months",
        })
    }
}

impl std::error::Error for SavingsError {}

pub fn validate_terms(share_bps: u32, term_months: u32) -> Result<(), SavingsError> {
    if !(1..=10_000).contains(&share_bps) {
        return Err(SavingsError::InvalidShare);
    }
    if !(1..=MAX_TERM_MONTHS).contains(&term_months) {
        return Err(SavingsError::InvalidTerm);
    }
    Ok(())
}

/// The last day the agreement covers: the day before the same calendar day `term_months`
/// later (a 36-month term starting 2026-04-01 ends 2029-03-31). When the end month has no
/// such day (a start on the 29th–31st), the term ends on that month's last day.
pub fn term_end(start: Day, term_months: u32) -> Option<Day> {
    let months = i64::from(start.year) * 12 + i64::from(start.month - 1) + i64::from(term_months);
    let year = i32::try_from(months.div_euclid(12)).ok()?;
    let month = u32::try_from(months.rem_euclid(12)).ok()? + 1;
    match Day::new(year, month, start.day) {
        Some(same) => day_before(same),
        None => (1..start.day)
            .rev()
            .find_map(|day| Day::new(year, month, day)),
    }
}

fn day_before(d: Day) -> Option<Day> {
    if d.day > 1 {
        return Day::new(d.year, d.month, d.day - 1);
    }
    let (y, m) = if d.month == 1 {
        (d.year - 1, 12)
    } else {
        (d.year, d.month - 1)
    };
    (1..=31).rev().find_map(|day| Day::new(y, m, day))
}

/// True when an order placed on `on` falls inside the agreement's term.
pub fn in_term(start: Day, end: Day, on: Day) -> bool {
    start.epoch_days() <= on.epoch_days() && on.epoch_days() <= end.epoch_days()
}

/// The reference price for one item: what the factory paid before we bought it for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Baseline {
    pub unit_price_milli: i64,
    pub currency: Currency,
    /// Normalised unit text ("" when the history did not say); compared case-insensitively.
    pub unit: String,
}

/// One order line we placed for the factory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderLine {
    pub unit_price: Money,
    pub quantity: u32,
    pub unit: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Share {
    /// (baseline − price) × quantity, floored at 0, rounded down to a minor unit.
    pub saving: Money,
    /// `saving` × share, rounded down to a minor unit.
    pub fee: Money,
}

fn unit_matches(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    a.is_empty() || b.is_empty() || a.eq_ignore_ascii_case(b)
}

pub fn share(baseline: &Baseline, line: &OrderLine, share_bps: u32) -> Result<Share, SavingsError> {
    if !(1..=10_000).contains(&share_bps) {
        return Err(SavingsError::InvalidShare);
    }
    let currency = line.unit_price.currency();
    if baseline.currency != currency {
        return Err(SavingsError::CurrencyMismatch);
    }
    if !unit_matches(&baseline.unit, &line.unit) {
        return Err(SavingsError::UnitMismatch);
    }
    let price_milli = i128::from(line.unit_price.minor()) * 1000;
    let per_unit = i128::from(baseline.unit_price_milli) - price_milli;
    if per_unit <= 0 {
        let zero = Money::zero(currency);
        return Ok(Share {
            saving: zero,
            fee: zero,
        });
    }
    let saving_milli = per_unit * i128::from(line.quantity);
    let saving = saving_milli / 1000;
    let fee = saving * i128::from(share_bps) / 10_000;
    let to_money = |v: i128| {
        i64::try_from(v)
            .map(|m| Money::from_minor(m, currency))
            .map_err(|_| SavingsError::Overflow)
    };
    Ok(Share {
        saving: to_money(saving)?,
        fee: to_money(fee)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> Day {
        Day::new(y, m, day).unwrap()
    }

    fn base(milli: i64) -> Baseline {
        Baseline {
            unit_price_milli: milli,
            currency: Currency::Jpy,
            unit: "pcs".into(),
        }
    }

    fn line(minor: i64, qty: u32) -> OrderLine {
        OrderLine {
            unit_price: Money::from_minor(minor, Currency::Jpy),
            quantity: qty,
            unit: "PCS".into(),
        }
    }

    #[test]
    fn half_of_the_drop() {
        // Paid ¥1,200 before, ¥1,000 now, 10 pieces: saving ¥2,000, our fee ¥1,000.
        let s = share(&base(1_200_000), &line(1_000, 10), DEFAULT_SHARE_BPS).unwrap();
        assert_eq!(s.saving.minor(), 2_000);
        assert_eq!(s.fee.minor(), 1_000);
    }

    #[test]
    fn no_drop_no_fee() {
        for price in [1_200, 1_500] {
            let s = share(&base(1_200_000), &line(price, 10), DEFAULT_SHARE_BPS).unwrap();
            assert_eq!((s.saving.minor(), s.fee.minor()), (0, 0));
        }
    }

    #[test]
    fn fractions_round_down_for_the_factory() {
        // Baseline ¥100.5, price ¥100, 3 pieces: saving ¥1.5 → ¥1, fee ¥0.5 → ¥0.
        let s = share(&base(100_500), &line(100, 3), DEFAULT_SHARE_BPS).unwrap();
        assert_eq!((s.saving.minor(), s.fee.minor()), (1, 0));
        let s = share(&base(100_500), &line(100, 7), DEFAULT_SHARE_BPS).unwrap();
        assert_eq!((s.saving.minor(), s.fee.minor()), (3, 1));
    }

    #[test]
    fn mismatches_are_refused() {
        let mut b = base(1_000_000);
        b.currency = Currency::Usd;
        assert_eq!(
            share(&b, &line(900, 1), 5_000),
            Err(SavingsError::CurrencyMismatch)
        );
        let mut b = base(1_000_000);
        b.unit = "box".into();
        assert_eq!(
            share(&b, &line(900, 1), 5_000),
            Err(SavingsError::UnitMismatch)
        );
        let mut b = base(1_000_000);
        b.unit = String::new();
        assert!(share(&b, &line(900, 1), 5_000).is_ok());
        assert_eq!(
            share(&base(1), &line(0, 1), 0),
            Err(SavingsError::InvalidShare)
        );
    }

    #[test]
    fn large_values_do_not_wrap() {
        let s = share(&base(i64::MAX), &line(0, u32::MAX), 10_000);
        assert_eq!(s, Err(SavingsError::Overflow));
    }

    #[test]
    fn term_ends() {
        assert_eq!(term_end(d(2026, 4, 1), 36), Some(d(2029, 3, 31)));
        assert_eq!(term_end(d(2026, 11, 15), 36), Some(d(2029, 11, 14)));
        assert_eq!(term_end(d(2027, 1, 31), 1), Some(d(2027, 2, 28)));
        assert_eq!(term_end(d(2026, 1, 1), 12), Some(d(2026, 12, 31)));
        let (s, e) = (d(2026, 4, 1), d(2029, 3, 31));
        assert!(in_term(s, e, s) && in_term(s, e, e));
        assert!(!in_term(s, e, d(2029, 4, 1)) && !in_term(s, e, d(2026, 3, 31)));
    }

    #[test]
    fn terms_validated() {
        assert!(validate_terms(5_000, 36).is_ok());
        assert_eq!(validate_terms(0, 36), Err(SavingsError::InvalidShare));
        assert_eq!(validate_terms(10_001, 36), Err(SavingsError::InvalidShare));
        assert_eq!(validate_terms(5_000, 0), Err(SavingsError::InvalidTerm));
    }
}
