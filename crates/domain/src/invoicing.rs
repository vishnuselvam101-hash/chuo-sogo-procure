//! Invoices Chuo-Sogo issues (Japanese qualified invoices, 適格請求書) and credit notes
//! (返還インボイス). Deterministic: totals are summed per tax rate and the tax is rounded once per
//! rate per invoice; credit notes can never return more than the original invoice per rate.

use crate::matching::{TaxRounding, tax};
use crate::money::{Currency, Money, MoneyError};

/// Standard and reduced consumption tax rates, and tax-exempt lines (basis points).
pub const STANDARD_RATE_BPS: u32 = 1000;
pub const REDUCED_RATE_BPS: u32 = 800;
pub const EXEMPT_RATE_BPS: u32 = 0;

pub fn is_allowed_rate(rate_bps: u32) -> bool {
    matches!(
        rate_bps,
        STANDARD_RATE_BPS | REDUCED_RATE_BPS | EXEMPT_RATE_BPS
    )
}

/// A qualified-invoice issuer registration number: `T` + 13-digit number whose first digit
/// is the check digit of the corporate number scheme (法人番号): `9 − (Σ Pn·Qn mod 9)` over
/// the other 12 digits counted from the right, with Qn = 1 for odd n and 2 for even n.
pub fn registration_number_valid(s: &str) -> bool {
    let Some(digits) = s.strip_prefix('T') else {
        return false;
    };
    if digits.len() != 13 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let d: Vec<u32> = digits.bytes().map(|b| u32::from(b - b'0')).collect();
    let sum: u32 = d[1..]
        .iter()
        .rev()
        .enumerate()
        .map(|(i, p)| p * if i % 2 == 0 { 1 } else { 2 })
        .sum();
    d[0] == 9 - sum % 9
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub quantity: u32,
    pub unit_price: Money,
    pub tax_rate_bps: u32,
}

impl Line {
    pub fn amount(&self) -> Result<Money, MoneyError> {
        if self.unit_price.minor() < 0 {
            return Err(MoneyError::Negative);
        }
        self.unit_price.checked_mul_qty(self.quantity)
    }
}

/// Base and tax for one rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateTotal {
    pub rate_bps: u32,
    pub base: Money,
    pub tax: Money,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Totals {
    /// Highest rate first (10 %, 8 %, 0 %), only rates that occur.
    pub per_rate: Vec<RateTotal>,
    pub subtotal: Money,
    pub tax: Money,
    pub total: Money,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvoiceError {
    NoLines,
    TooManyLines,
    Quantity,
    RateNotAllowed(u32),
    CurrencyMismatch,
    Money(MoneyError),
    /// A credit note would return more than was invoiced (and not yet credited) at this rate.
    ExceedsOriginal {
        rate_bps: u32,
    },
}

impl From<MoneyError> for InvoiceError {
    fn from(e: MoneyError) -> Self {
        InvoiceError::Money(e)
    }
}

pub const MAX_LINES: usize = 200;

/// Totals for an invoice: lines summed per rate, tax rounded once per rate.
pub fn totals(
    lines: &[Line],
    currency: Currency,
    rounding: TaxRounding,
) -> Result<Totals, InvoiceError> {
    if lines.is_empty() {
        return Err(InvoiceError::NoLines);
    }
    if lines.len() > MAX_LINES {
        return Err(InvoiceError::TooManyLines);
    }
    let mut per_rate: Vec<RateTotal> = Vec::new();
    for l in lines {
        if !(1..=1_000_000_000).contains(&l.quantity) {
            return Err(InvoiceError::Quantity);
        }
        if !is_allowed_rate(l.tax_rate_bps) {
            return Err(InvoiceError::RateNotAllowed(l.tax_rate_bps));
        }
        if l.unit_price.currency() != currency {
            return Err(InvoiceError::CurrencyMismatch);
        }
        let amount = l.amount()?;
        match per_rate.iter_mut().find(|r| r.rate_bps == l.tax_rate_bps) {
            Some(r) => r.base = r.base.checked_add(amount)?,
            None => per_rate.push(RateTotal {
                rate_bps: l.tax_rate_bps,
                base: amount,
                tax: Money::zero(currency),
            }),
        }
    }
    per_rate.sort_by_key(|r| std::cmp::Reverse(r.rate_bps));
    let mut subtotal = Money::zero(currency);
    let mut total_tax = Money::zero(currency);
    for r in &mut per_rate {
        r.tax = tax(r.base, r.rate_bps, rounding)?;
        subtotal = subtotal.checked_add(r.base)?;
        total_tax = total_tax.checked_add(r.tax)?;
    }
    Ok(Totals {
        per_rate,
        subtotal,
        tax: total_tax,
        total: subtotal.checked_add(total_tax)?,
    })
}

/// Totals for tax-inclusive prices (consumer sales, 総額表示): lines are summed per rate
/// including tax, and the tax contained is computed once per rate as
/// `total × rate / (10 000 + rate)`, rounded with `rounding`; the base is the rest.
pub fn totals_inclusive(
    lines: &[Line],
    currency: Currency,
    rounding: TaxRounding,
) -> Result<Totals, InvoiceError> {
    // Same validation and per-rate grouping as the exclusive case, with tax set below.
    let gross = totals(lines, currency, rounding)?;
    let mut per_rate = Vec::with_capacity(gross.per_rate.len());
    let mut subtotal = Money::zero(currency);
    let mut total_tax = Money::zero(currency);
    for r in &gross.per_rate {
        let incl = i128::from(r.base.minor());
        let num = incl * i128::from(r.rate_bps);
        let den = 10_000 + i128::from(r.rate_bps);
        let (q, rem) = (num / den, num % den);
        let contained = match rounding {
            TaxRounding::Floor => q,
            TaxRounding::Ceil => q + i128::from(rem > 0),
            TaxRounding::HalfUp => q + i128::from(rem * 2 >= den),
        };
        let tax = Money::from_minor(
            i64::try_from(contained).map_err(|_| MoneyError::Overflow)?,
            currency,
        );
        let base = Money::from_minor(r.base.minor() - tax.minor(), currency);
        subtotal = subtotal.checked_add(base)?;
        total_tax = total_tax.checked_add(tax)?;
        per_rate.push(RateTotal {
            rate_bps: r.rate_bps,
            base,
            tax,
        });
    }
    Ok(Totals {
        per_rate,
        subtotal,
        tax: total_tax,
        total: gross.subtotal,
    })
}

/// A credit note may return at most what the original invoice charged at each rate, minus
/// what earlier credit notes already returned at that rate — for the base and for the tax.
/// Returns the credit's per-rate totals with each tax capped at the tax still returnable
/// (rounding each credit note separately could otherwise return more tax than was charged).
pub fn limit_credit(
    original: &[RateTotal],
    already_credited: &[RateTotal],
    credit: &[RateTotal],
) -> Result<Vec<RateTotal>, InvoiceError> {
    let mut out = Vec::with_capacity(credit.len());
    for c in credit {
        let charged = original.iter().find(|o| o.rate_bps == c.rate_bps).ok_or(
            InvoiceError::ExceedsOriginal {
                rate_bps: c.rate_bps,
            },
        )?;
        let mut base_used = c.base;
        let mut tax_returned = Money::zero(c.tax.currency());
        for a in already_credited.iter().filter(|a| a.rate_bps == c.rate_bps) {
            base_used = base_used.checked_add(a.base)?;
            tax_returned = tax_returned.checked_add(a.tax)?;
        }
        if base_used.minor() > charged.base.minor() {
            return Err(InvoiceError::ExceedsOriginal {
                rate_bps: c.rate_bps,
            });
        }
        let tax_left = (charged.tax.minor() - tax_returned.minor()).max(0);
        out.push(RateTotal {
            rate_bps: c.rate_bps,
            base: c.base,
            tax: Money::from_minor(c.tax.minor().min(tax_left), c.tax.currency()),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yen(m: i64) -> Money {
        Money::from_minor(m, Currency::Jpy)
    }
    fn line(q: u32, p: i64, r: u32) -> Line {
        Line {
            quantity: q,
            unit_price: yen(p),
            tax_rate_bps: r,
        }
    }

    #[test]
    fn registration_numbers() {
        // National Tax Agency's own corporate number.
        assert!(registration_number_valid("T7000012050002"));
        assert!(!registration_number_valid("T8000012050002"));
        assert!(!registration_number_valid("7000012050002"));
        assert!(!registration_number_valid("T700001205000"));
        assert!(!registration_number_valid("T70000120500a2"));
        assert!(!registration_number_valid("t7000012050002"));
    }

    #[test]
    fn tax_is_rounded_once_per_rate_not_per_line() {
        // Three lines of ¥333 at 10 %: per line 33.3 → 33 ×3 = 99; per rate 999 → 99.9 → 99.
        // Two lines of ¥105 at 8 %: 8.4 per line; per rate 210 × 8 % = 16.8 → 16 (floor).
        let t = totals(
            &[
                line(1, 333, 1000),
                line(2, 333, 1000),
                line(1, 105, 800),
                line(1, 105, 800),
                line(1, 500, 0),
            ],
            Currency::Jpy,
            TaxRounding::Floor,
        )
        .unwrap();
        assert_eq!(
            t.per_rate,
            vec![
                RateTotal {
                    rate_bps: 1000,
                    base: yen(999),
                    tax: yen(99)
                },
                RateTotal {
                    rate_bps: 800,
                    base: yen(210),
                    tax: yen(16)
                },
                RateTotal {
                    rate_bps: 0,
                    base: yen(500),
                    tax: yen(0)
                },
            ]
        );
        assert_eq!(t.subtotal, yen(1709));
        assert_eq!(t.tax, yen(115));
        assert_eq!(t.total, yen(1824));
        let up = totals(&[line(2, 105, 800)], Currency::Jpy, TaxRounding::HalfUp).unwrap();
        assert_eq!(up.tax, yen(17));
    }

    #[test]
    fn inclusive_prices_contain_the_tax_once_per_rate() {
        // ¥1,100 incl. 10 % ×3 = ¥3,300 → tax ¥300; ¥1,080 incl. 8 % → tax ¥80.
        let t = totals_inclusive(
            &[line(3, 1100, 1000), line(1, 1080, 800)],
            Currency::Jpy,
            TaxRounding::Floor,
        )
        .unwrap();
        assert_eq!(t.total, yen(4380));
        assert_eq!(t.tax, yen(380));
        assert_eq!(t.subtotal, yen(4000));
        assert_eq!(t.per_rate[0].base, yen(3000));
        // ¥999 incl. 10 %: 90.81… → 90 (floor), base 909.
        let t = totals_inclusive(&[line(1, 999, 1000)], Currency::Jpy, TaxRounding::Floor).unwrap();
        assert_eq!((t.tax, t.subtotal, t.total), (yen(90), yen(909), yen(999)));
        let t =
            totals_inclusive(&[line(1, 999, 1000)], Currency::Jpy, TaxRounding::HalfUp).unwrap();
        assert_eq!(t.tax, yen(91));
    }

    #[test]
    fn invalid_invoices_are_refused() {
        let jpy = Currency::Jpy;
        let r = TaxRounding::Floor;
        assert_eq!(totals(&[], jpy, r), Err(InvoiceError::NoLines));
        assert_eq!(
            totals(&[line(1, 100, 500)], jpy, r),
            Err(InvoiceError::RateNotAllowed(500))
        );
        assert_eq!(
            totals(&[line(0, 100, 1000)], jpy, r),
            Err(InvoiceError::Quantity)
        );
        let usd = Line {
            quantity: 1,
            unit_price: Money::from_minor(100, Currency::Usd),
            tax_rate_bps: 1000,
        };
        assert_eq!(totals(&[usd], jpy, r), Err(InvoiceError::CurrencyMismatch));
        assert!(matches!(
            totals(&[line(1_000_000_000, i64::MAX / 2, 1000)], jpy, r),
            Err(InvoiceError::Money(_))
        ));
        assert_eq!(
            totals(&[line(1, -1, 1000)], jpy, r),
            Err(InvoiceError::Money(MoneyError::Negative))
        );
    }

    #[test]
    fn credit_notes_cannot_exceed_the_original_per_rate() {
        let rt = |rate_bps, base| RateTotal {
            rate_bps,
            base: yen(base),
            tax: yen(0),
        };
        let check_credit =
            |o: &[RateTotal], a: &[RateTotal], c: &[RateTotal]| limit_credit(o, a, c).map(|_| ());
        let original = [rt(1000, 10_000), rt(800, 2_000)];
        assert!(check_credit(&original, &[], &[rt(1000, 10_000)]).is_ok());
        assert!(check_credit(&original, &[rt(1000, 4_000)], &[rt(1000, 6_000)]).is_ok());
        assert_eq!(
            check_credit(&original, &[rt(1000, 4_000)], &[rt(1000, 6_001)]),
            Err(InvoiceError::ExceedsOriginal { rate_bps: 1000 })
        );
        // Returning 8 % goods may not be booked against the 10 % amount.
        assert_eq!(
            check_credit(&original, &[], &[rt(800, 2_001)]),
            Err(InvoiceError::ExceedsOriginal { rate_bps: 800 })
        );
        assert_eq!(
            check_credit(&original, &[], &[rt(0, 1)]),
            Err(InvoiceError::ExceedsOriginal { rate_bps: 0 })
        );
    }

    #[test]
    fn credit_notes_never_return_more_tax_than_was_charged() {
        // Original: base 2 at 10 %, rounded up → tax 1. Two credits of base 1 would each
        // round up to tax 1; the second may only return what is left (0).
        let t = |base, tax| RateTotal {
            rate_bps: 1000,
            base: yen(base),
            tax: yen(tax),
        };
        let original = [t(2, 1)];
        assert_eq!(
            limit_credit(&original, &[], &[t(1, 1)]).unwrap(),
            vec![t(1, 1)]
        );
        assert_eq!(
            limit_credit(&original, &[t(1, 1)], &[t(1, 1)]).unwrap(),
            vec![t(1, 0)]
        );
    }
}
