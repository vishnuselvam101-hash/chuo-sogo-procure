//! Three-way match: what was ordered, what was received, what the supplier invoiced.
//! Deterministic and exact (no tolerance in v1): every difference becomes a discrepancy a
//! human resolves. Tax is recomputed from the invoice's rate and rounding rule.

use crate::money::{Currency, Money, MoneyError};

/// How the tax amount is rounded to minor units (Japanese qualified invoices commonly round
/// down once per invoice and rate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaxRounding {
    Floor,
    HalfUp,
    Ceil,
}

impl TaxRounding {
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "floor" => Some(TaxRounding::Floor),
            "half_up" => Some(TaxRounding::HalfUp),
            "ceil" => Some(TaxRounding::Ceil),
            _ => None,
        }
    }
}

/// `base × rate_bps / 10 000`, rounded once, in integer arithmetic.
pub fn tax(base: Money, rate_bps: u32, rounding: TaxRounding) -> Result<Money, MoneyError> {
    if base.minor() < 0 {
        return Err(MoneyError::Negative);
    }
    let num = i128::from(base.minor()) * i128::from(rate_bps);
    let q = num / 10_000;
    let r = num % 10_000;
    let rounded = match rounding {
        TaxRounding::Floor => q,
        TaxRounding::Ceil => q + i128::from(r > 0),
        TaxRounding::HalfUp => q + i128::from(r * 2 >= 10_000),
    };
    i64::try_from(rounded)
        .map(|m| Money::from_minor(m, base.currency()))
        .map_err(|_| MoneyError::Overflow)
}

#[derive(Debug, Clone)]
pub struct OrderLine<Id> {
    pub item: Id,
    pub quantity: u32,
    pub unit_price: Money,
}

#[derive(Debug, Clone)]
pub struct InvoiceLine<Id> {
    pub item: Id,
    pub quantity: u32,
    pub unit_price: Money,
    /// Tax rate of this line in basis points (Japan: 1000 standard, 800 reduced).
    pub tax_rate_bps: u32,
}

#[derive(Debug, Clone)]
pub struct Invoice<Id> {
    pub currency: Currency,
    pub lines: Vec<InvoiceLine<Id>>,
    pub shipping: Money,
    /// Rate applied to shipping.
    pub shipping_tax_rate_bps: u32,
    pub tax_rounding: TaxRounding,
    /// Tax as printed on the invoice.
    pub tax: Money,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Discrepancy<Id> {
    CurrencyMismatch,
    /// The invoice bills an item that was not ordered.
    UnknownItem {
        item: Id,
    },
    /// The same item appears on several invoice lines.
    DuplicateLine {
        item: Id,
    },
    PriceMismatch {
        item: Id,
        ordered: Money,
        invoiced: Money,
    },
    /// Billed more than was ordered.
    QuantityOverOrdered {
        item: Id,
        ordered: u32,
        invoiced: u32,
    },
    /// Billed more than has been received.
    QuantityOverReceived {
        item: Id,
        received: u32,
        invoiced: u32,
    },
    /// Billed less than has been received: the rest would never be billed (one invoice per
    /// order in v1), so it must be looked at.
    QuantityUnderReceived {
        item: Id,
        received: u32,
        invoiced: u32,
    },
    /// Ordered and received but not on the invoice.
    NotInvoiced {
        item: Id,
    },
    ShippingMismatch {
        ordered: Money,
        invoiced: Money,
    },
    TaxMismatch {
        expected: Money,
        invoiced: Money,
    },
    Overflow,
}

impl<Id> Discrepancy<Id> {
    pub const fn code(&self) -> &'static str {
        match self {
            Discrepancy::CurrencyMismatch => "currency_mismatch",
            Discrepancy::UnknownItem { .. } => "unknown_item",
            Discrepancy::DuplicateLine { .. } => "duplicate_line",
            Discrepancy::PriceMismatch { .. } => "price_mismatch",
            Discrepancy::QuantityOverOrdered { .. } => "quantity_over_ordered",
            Discrepancy::QuantityOverReceived { .. } => "quantity_over_received",
            Discrepancy::QuantityUnderReceived { .. } => "quantity_under_received",
            Discrepancy::NotInvoiced { .. } => "not_invoiced",
            Discrepancy::ShippingMismatch { .. } => "shipping_mismatch",
            Discrepancy::TaxMismatch { .. } => "tax_mismatch",
            Discrepancy::Overflow => "amount_overflow",
        }
    }
}

/// Invoice totals as computed from its own lines (for display and the stored record).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Totals {
    pub subtotal: Money,
    pub expected_tax: Money,
    pub total: Money,
}

/// Tax is computed once per rate over the invoice (the Japanese qualified-invoice rule):
/// lines and shipping are grouped by rate, each group's base is taxed and rounded once.
pub fn totals<Id>(inv: &Invoice<Id>) -> Result<Totals, MoneyError> {
    let mut groups: Vec<(u32, Money)> = Vec::new();
    let mut add = |rate: u32, amount: Money| -> Result<(), MoneyError> {
        match groups.iter_mut().find(|(r, _)| *r == rate) {
            Some((_, base)) => *base = base.checked_add(amount)?,
            None => groups.push((rate, amount)),
        }
        Ok(())
    };
    let mut subtotal = Money::zero(inv.currency);
    for l in &inv.lines {
        let amount = l.unit_price.checked_mul_qty(l.quantity)?;
        subtotal = subtotal.checked_add(amount)?;
        add(l.tax_rate_bps, amount)?;
    }
    add(inv.shipping_tax_rate_bps, inv.shipping)?;
    let mut expected_tax = Money::zero(inv.currency);
    for (rate, base) in &groups {
        expected_tax = expected_tax.checked_add(tax(*base, *rate, inv.tax_rounding)?)?;
    }
    Ok(Totals {
        subtotal,
        expected_tax,
        total: subtotal.checked_add(inv.shipping)?.checked_add(inv.tax)?,
    })
}

/// Compares the invoice with the order and the quantities received so far.
/// `received` lists (item, total quantity received).
pub fn three_way<Id: PartialEq + Clone>(
    order: &[OrderLine<Id>],
    order_currency: Currency,
    order_shipping: Money,
    received: &[(Id, u32)],
    inv: &Invoice<Id>,
) -> Vec<Discrepancy<Id>> {
    let mut out = Vec::new();
    if inv.currency != order_currency {
        // Without FX nothing else can be compared meaningfully.
        out.push(Discrepancy::CurrencyMismatch);
        return out;
    }
    for (i, line) in inv.lines.iter().enumerate() {
        if inv.lines[..i].iter().any(|l| l.item == line.item) {
            out.push(Discrepancy::DuplicateLine {
                item: line.item.clone(),
            });
            continue;
        }
        let Some(ordered) = order.iter().find(|o| o.item == line.item) else {
            out.push(Discrepancy::UnknownItem {
                item: line.item.clone(),
            });
            continue;
        };
        if line.unit_price != ordered.unit_price {
            out.push(Discrepancy::PriceMismatch {
                item: line.item.clone(),
                ordered: ordered.unit_price,
                invoiced: line.unit_price,
            });
        }
        if line.quantity > ordered.quantity {
            out.push(Discrepancy::QuantityOverOrdered {
                item: line.item.clone(),
                ordered: ordered.quantity,
                invoiced: line.quantity,
            });
        }
        let got = received
            .iter()
            .find(|(id, _)| *id == line.item)
            .map_or(0, |(_, q)| *q);
        if line.quantity > got {
            out.push(Discrepancy::QuantityOverReceived {
                item: line.item.clone(),
                received: got,
                invoiced: line.quantity,
            });
        } else if line.quantity < got {
            out.push(Discrepancy::QuantityUnderReceived {
                item: line.item.clone(),
                received: got,
                invoiced: line.quantity,
            });
        }
    }
    for o in order {
        let got = received
            .iter()
            .find(|(id, _)| *id == o.item)
            .map_or(0, |(_, q)| *q);
        if got > 0 && !inv.lines.iter().any(|l| l.item == o.item) {
            out.push(Discrepancy::NotInvoiced {
                item: o.item.clone(),
            });
        }
    }
    if inv.shipping != order_shipping {
        out.push(Discrepancy::ShippingMismatch {
            ordered: order_shipping,
            invoiced: inv.shipping,
        });
    }
    match totals(inv) {
        Ok(t) if t.expected_tax != inv.tax => out.push(Discrepancy::TaxMismatch {
            expected: t.expected_tax,
            invoiced: inv.tax,
        }),
        Ok(_) => {}
        Err(_) => out.push(Discrepancy::Overflow),
    }
    out
}

// ---------------------------------------------------------------------------------------
// Tolerances (agent layer L2). `three_way` stays exact: every difference is a discrepancy.
// A tolerance only *classifies* a discrepancy afterwards, so the matching agent can propose
// accepting a small one; a person still decides each.

/// Per-organisation match tolerances. A zero limit is "not set". A family (price, quantity,
/// rounding) applies only when at least one of its limits is set, and a difference is within
/// tolerance only when it is within **every** limit set for its family (the stricter wins).
/// [`Tolerance::exact`] sets nothing: every discrepancy is out of tolerance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tolerance {
    /// Currency of the absolute limits. They never apply to another currency (no FX): a
    /// discrepancy in another currency fails any absolute limit that is set.
    pub currency: Currency,
    /// Unit price may differ from the ordered price by at most this share of it (bps).
    pub price_bps: u32,
    /// The line's difference (unit price difference × invoiced quantity) at most this.
    pub price_line_abs: Money,
    /// Invoiced quantity may differ from the ordered/received quantity by at most this share.
    pub quantity_bps: u32,
    /// ... and by at most this many units.
    pub quantity_abs: u32,
    /// Printed tax may differ from the recomputed tax by at most this (rounding).
    pub rounding: Money,
}

impl Tolerance {
    pub const fn exact(currency: Currency) -> Self {
        Tolerance {
            currency,
            price_bps: 0,
            price_line_abs: Money::zero(currency),
            quantity_bps: 0,
            quantity_abs: 0,
            rounding: Money::zero(currency),
        }
    }
}

/// What the matching agent proposes for one discrepancy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classification {
    /// Small enough to accept as invoiced.
    WithinTolerance,
    /// Out of tolerance and the supplier billed too much: ask for a credit note.
    CreditNote,
    /// Out of tolerance but not an overcharge (billed less, missing lines, other currency):
    /// a person looks at it.
    Review,
}

impl Classification {
    pub const fn code(self) -> &'static str {
        match self {
            Classification::WithinTolerance => "within_tolerance",
            Classification::CreditNote => "out_of_tolerance",
            Classification::Review => "needs_review",
        }
    }
}

/// |a − b| of two amounts in the same currency.
fn abs_diff(a: Money, b: Money) -> Option<i128> {
    (a.currency() == b.currency()).then(|| (i128::from(a.minor()) - i128::from(b.minor())).abs())
}

/// `diff ≤ base × bps / 10 000`, exactly.
fn within_share(diff: i128, base: i128, bps: u32) -> bool {
    diff * 10_000 <= base.abs() * i128::from(bps)
}

/// `diff ≤ limit` for an absolute limit in `limit`'s currency; a difference in another
/// currency never fits.
fn within_abs(diff: i128, currency: Currency, limit: Money) -> bool {
    currency == limit.currency() && diff <= i128::from(limit.minor())
}

fn quantity_within(reference: u32, invoiced: u32, tol: &Tolerance) -> bool {
    if tol.quantity_bps == 0 && tol.quantity_abs == 0 {
        return false;
    }
    let diff = (i128::from(reference) - i128::from(invoiced)).abs();
    (tol.quantity_bps == 0 || within_share(diff, i128::from(reference), tol.quantity_bps))
        && (tol.quantity_abs == 0 || diff <= i128::from(tol.quantity_abs))
}

/// The amount at stake in a discrepancy (minor units), when it has one: a price
/// difference × the invoiced quantity, a shipping or tax difference.
pub fn difference<Id>(d: &Discrepancy<Id>, invoiced_quantity: u32) -> Option<Money> {
    let (a, b, qty) = match d {
        Discrepancy::PriceMismatch {
            ordered, invoiced, ..
        } => (*ordered, *invoiced, invoiced_quantity),
        Discrepancy::ShippingMismatch { ordered, invoiced } => (*ordered, *invoiced, 1),
        Discrepancy::TaxMismatch { expected, invoiced } => (*expected, *invoiced, 1),
        _ => return None,
    };
    let diff = abs_diff(a, b)? * i128::from(qty);
    i64::try_from(diff)
        .ok()
        .map(|m| Money::from_minor(m, b.currency()))
}

/// Classifies one discrepancy. `invoiced_quantity` is the invoiced quantity of the line a
/// price difference is on (ignored otherwise). Pure and exact (integer arithmetic only).
pub fn classify<Id>(
    d: &Discrepancy<Id>,
    invoiced_quantity: u32,
    tol: &Tolerance,
) -> Classification {
    let within = match d {
        Discrepancy::PriceMismatch {
            ordered, invoiced, ..
        } => {
            if tol.price_bps == 0 && tol.price_line_abs.minor() == 0 {
                false
            } else {
                match abs_diff(*ordered, *invoiced) {
                    None => false,
                    Some(unit) => {
                        let line = unit * i128::from(invoiced_quantity);
                        (tol.price_bps == 0
                            || within_share(unit, i128::from(ordered.minor()), tol.price_bps))
                            && (tol.price_line_abs.minor() == 0
                                || within_abs(line, invoiced.currency(), tol.price_line_abs))
                    }
                }
            }
        }
        Discrepancy::QuantityOverOrdered {
            ordered, invoiced, ..
        } => quantity_within(*ordered, *invoiced, tol),
        Discrepancy::QuantityOverReceived {
            received, invoiced, ..
        }
        | Discrepancy::QuantityUnderReceived {
            received, invoiced, ..
        } => quantity_within(*received, *invoiced, tol),
        Discrepancy::TaxMismatch { expected, invoiced } => {
            tol.rounding.minor() > 0
                && abs_diff(*expected, *invoiced)
                    .is_some_and(|diff| within_abs(diff, invoiced.currency(), tol.rounding))
        }
        // Never a matter of tolerance.
        Discrepancy::CurrencyMismatch
        | Discrepancy::UnknownItem { .. }
        | Discrepancy::DuplicateLine { .. }
        | Discrepancy::NotInvoiced { .. }
        | Discrepancy::ShippingMismatch { .. }
        | Discrepancy::Overflow => false,
    };
    if within {
        return Classification::WithinTolerance;
    }
    let overbilled = match d {
        Discrepancy::PriceMismatch {
            ordered, invoiced, ..
        }
        | Discrepancy::ShippingMismatch { ordered, invoiced } => invoiced.minor() > ordered.minor(),
        Discrepancy::TaxMismatch { expected, invoiced } => invoiced.minor() > expected.minor(),
        Discrepancy::QuantityOverOrdered { .. }
        | Discrepancy::QuantityOverReceived { .. }
        | Discrepancy::UnknownItem { .. }
        | Discrepancy::DuplicateLine { .. } => true,
        Discrepancy::QuantityUnderReceived { .. }
        | Discrepancy::NotInvoiced { .. }
        | Discrepancy::CurrencyMismatch
        | Discrepancy::Overflow => false,
    };
    if overbilled {
        Classification::CreditNote
    } else {
        Classification::Review
    }
}

impl<Id> Discrepancy<Id> {
    /// Rebuilds a discrepancy from its stored form (`discrepancies.kind`, `expected`,
    /// `actual`, the invoice currency). `None` for an unknown kind or missing values.
    pub fn from_stored(
        code: &str,
        item: Id,
        expected: Option<i64>,
        actual: Option<i64>,
        currency: Currency,
    ) -> Option<Self> {
        let money = |v: Option<i64>| v.map(|m| Money::from_minor(m, currency));
        let qty = |v: Option<i64>| v.and_then(|q| u32::try_from(q).ok());
        Some(match code {
            "currency_mismatch" => Discrepancy::CurrencyMismatch,
            "unknown_item" => Discrepancy::UnknownItem { item },
            "duplicate_line" => Discrepancy::DuplicateLine { item },
            "not_invoiced" => Discrepancy::NotInvoiced { item },
            "amount_overflow" => Discrepancy::Overflow,
            "price_mismatch" => Discrepancy::PriceMismatch {
                item,
                ordered: money(expected)?,
                invoiced: money(actual)?,
            },
            "quantity_over_ordered" => Discrepancy::QuantityOverOrdered {
                item,
                ordered: qty(expected)?,
                invoiced: qty(actual)?,
            },
            "quantity_over_received" => Discrepancy::QuantityOverReceived {
                item,
                received: qty(expected)?,
                invoiced: qty(actual)?,
            },
            "quantity_under_received" => Discrepancy::QuantityUnderReceived {
                item,
                received: qty(expected)?,
                invoiced: qty(actual)?,
            },
            "shipping_mismatch" => Discrepancy::ShippingMismatch {
                ordered: money(expected)?,
                invoiced: money(actual)?,
            },
            "tax_mismatch" => Discrepancy::TaxMismatch {
                expected: money(expected)?,
                invoiced: money(actual)?,
            },
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JPY: Currency = Currency::Jpy;
    fn yen(n: i64) -> Money {
        Money::from_minor(n, JPY)
    }

    fn order() -> Vec<OrderLine<u32>> {
        vec![
            OrderLine {
                item: 1,
                quantity: 100,
                unit_price: yen(1200),
            },
            OrderLine {
                item: 2,
                quantity: 4,
                unit_price: yen(45_000),
            },
        ]
    }

    fn invoice(lines: Vec<(u32, u32, i64)>, shipping: i64, tax_minor: i64) -> Invoice<u32> {
        Invoice {
            currency: JPY,
            lines: lines
                .into_iter()
                .map(|(item, quantity, p)| InvoiceLine {
                    item,
                    quantity,
                    unit_price: yen(p),
                    tax_rate_bps: 1000,
                })
                .collect(),
            shipping: yen(shipping),
            shipping_tax_rate_bps: 1000,
            tax_rounding: TaxRounding::Floor,
            tax: yen(tax_minor),
        }
    }

    #[test]
    fn tax_rounding_is_exact() {
        // ¥12,345 × 10% = 1,234.5
        assert_eq!(tax(yen(12_345), 1000, TaxRounding::Floor), Ok(yen(1234)));
        assert_eq!(tax(yen(12_345), 1000, TaxRounding::HalfUp), Ok(yen(1235)));
        assert_eq!(tax(yen(12_345), 1000, TaxRounding::Ceil), Ok(yen(1235)));
        assert_eq!(tax(yen(12_340), 1000, TaxRounding::Ceil), Ok(yen(1234)));
        // 8% reduced rate.
        assert_eq!(tax(yen(999), 800, TaxRounding::Floor), Ok(yen(79)));
        assert_eq!(
            tax(yen(i64::MAX), 1000, TaxRounding::Floor).map(|m| m.minor()),
            Ok(i64::MAX / 10)
        );
        assert_eq!(
            tax(yen(-1), 1000, TaxRounding::Floor),
            Err(MoneyError::Negative)
        );
    }

    #[test]
    fn clean_invoice_matches() {
        // 120,000 + 180,000 + shipping 2,000 = 302,000; tax 10% = 30,200.
        let inv = invoice(vec![(1, 100, 1200), (2, 4, 45_000)], 2000, 30_200);
        let d = three_way(&order(), JPY, yen(2000), &[(1, 100), (2, 4)], &inv);
        assert!(d.is_empty(), "{d:?}");
        assert_eq!(totals(&inv).unwrap().total, yen(332_200));
    }

    #[test]
    fn every_difference_is_reported() {
        let inv = invoice(
            vec![(1, 110, 1250), (3, 1, 10), (2, 4, 45_000), (2, 1, 1)],
            2500,
            1,
        );
        let d = three_way(&order(), JPY, yen(2000), &[(1, 100), (2, 3)], &inv);
        let codes: Vec<_> = d.iter().map(Discrepancy::code).collect();
        assert_eq!(
            codes,
            vec![
                "price_mismatch",
                "quantity_over_ordered",
                "quantity_over_received",
                "unknown_item",
                "quantity_over_received",
                "duplicate_line",
                "shipping_mismatch",
                "tax_mismatch",
            ]
        );
    }

    #[test]
    fn invoicing_less_than_received_or_skipping_lines_is_flagged() {
        // Received everything, billed only half of line 1 and nothing for line 2.
        let inv = invoice(vec![(1, 50, 1200)], 2000, 6200);
        let d = three_way(&order(), JPY, yen(2000), &[(1, 100), (2, 4)], &inv);
        assert_eq!(
            d,
            vec![
                Discrepancy::QuantityUnderReceived {
                    item: 1,
                    received: 100,
                    invoiced: 50
                },
                Discrepancy::NotInvoiced { item: 2 },
            ]
        );
    }

    #[test]
    fn mixed_tax_rates_are_taxed_per_rate_and_rounded_once_each() {
        // Line 1 at 8% (¥120,000 → 9,600), line 2 + shipping at 10% (¥182,000 → 18,200).
        let mut inv = invoice(vec![(1, 100, 1200), (2, 4, 45_000)], 2000, 27_800);
        inv.lines[0].tax_rate_bps = 800;
        let d = three_way(&order(), JPY, yen(2000), &[(1, 100), (2, 4)], &inv);
        assert!(d.is_empty(), "{d:?}");
        // Per rate, not per line: ¥999 at 10% → 99; ¥49 at 8% → 3.92 → 3; total 102.
        let small = Invoice {
            currency: JPY,
            lines: vec![
                InvoiceLine {
                    item: 1,
                    quantity: 3,
                    unit_price: yen(333),
                    tax_rate_bps: 1000,
                },
                InvoiceLine {
                    item: 2,
                    quantity: 7,
                    unit_price: yen(7),
                    tax_rate_bps: 800,
                },
            ],
            shipping: yen(0),
            shipping_tax_rate_bps: 1000,
            tax_rounding: TaxRounding::Floor,
            tax: yen(102),
        };
        assert_eq!(totals(&small).unwrap().expected_tax, yen(102));
    }

    #[test]
    fn other_currency_is_never_converted() {
        let mut inv = invoice(vec![(1, 100, 1200)], 2000, 0);
        inv.currency = Currency::Usd;
        assert_eq!(
            three_way(&order(), JPY, yen(2000), &[(1, 100)], &inv),
            vec![Discrepancy::CurrencyMismatch]
        );
    }

    fn tol() -> Tolerance {
        Tolerance {
            currency: JPY,
            price_bps: 100, // 1%
            price_line_abs: yen(1000),
            quantity_bps: 0,
            quantity_abs: 2,
            rounding: yen(1),
        }
    }

    fn price(ordered: i64, invoiced: i64) -> Discrepancy<u32> {
        Discrepancy::PriceMismatch {
            item: 1,
            ordered: yen(ordered),
            invoiced: yen(invoiced),
        }
    }

    #[test]
    fn exact_tolerance_accepts_nothing_three_way_reports() {
        // Zero tolerance changes nothing: every discrepancy three_way reports stays out.
        let inv = invoice(
            vec![(1, 110, 1201), (3, 1, 10), (2, 4, 45_000), (2, 1, 1)],
            2500,
            1,
        );
        let found = three_way(&order(), JPY, yen(2000), &[(1, 100), (2, 3)], &inv);
        assert_eq!(found.len(), 8);
        let exact = Tolerance::exact(JPY);
        for d in &found {
            assert_ne!(
                classify(d, 110, &exact),
                Classification::WithinTolerance,
                "{d:?}"
            );
        }
        assert_eq!(
            classify(&price(1200, 1201), 1, &exact),
            Classification::CreditNote
        );
        assert_eq!(
            classify(&price(1200, 1199), 1, &exact),
            Classification::Review
        );
    }

    #[test]
    fn price_tolerance_needs_every_limit_set() {
        let t = tol();
        // 1% of 1,200 = 12 per unit; line limit ¥1,000.
        assert_eq!(
            classify(&price(1200, 1212), 50, &t),
            Classification::WithinTolerance
        );
        assert_eq!(
            classify(&price(1200, 1213), 50, &t),
            Classification::CreditNote
        );
        // Within 1% but the line difference (12 × 100 = 1,200) exceeds ¥1,000.
        assert_eq!(
            classify(&price(1200, 1212), 100, &t),
            Classification::CreditNote
        );
        // Undercharges within tolerance are accepted too; outside they need review.
        assert_eq!(
            classify(&price(1200, 1190), 10, &t),
            Classification::WithinTolerance
        );
        assert_eq!(classify(&price(1200, 1000), 10, &t), Classification::Review);
        // Only a percentage set: the line amount does not matter.
        let pct = Tolerance {
            price_line_abs: yen(0),
            ..t
        };
        assert_eq!(
            classify(&price(1200, 1212), 1_000_000, &pct),
            Classification::WithinTolerance
        );
        // Only an absolute limit set.
        let abs = Tolerance { price_bps: 0, ..t };
        assert_eq!(
            classify(&price(1200, 1300), 10, &abs),
            Classification::WithinTolerance
        );
        assert_eq!(
            classify(&price(1200, 1301), 10, &abs),
            Classification::CreditNote
        );
        // Boundary is exact in integers: 0.5% of 999 = 4.995, so 4 fits and 5 does not.
        let half = Tolerance {
            price_bps: 50,
            price_line_abs: yen(0),
            ..t
        };
        assert_eq!(
            classify(&price(999, 1003), 1, &half),
            Classification::WithinTolerance
        );
        assert_eq!(
            classify(&price(999, 1004), 1, &half),
            Classification::CreditNote
        );
    }

    #[test]
    fn absolute_limits_never_apply_to_another_currency() {
        let usd = |n| Money::from_minor(n, Currency::Usd);
        let d = Discrepancy::PriceMismatch {
            item: 1,
            ordered: usd(1000),
            invoiced: usd(1001),
        };
        assert_eq!(classify(&d, 1, &tol()), Classification::CreditNote);
        let pct_only = Tolerance {
            price_line_abs: yen(0),
            ..tol()
        };
        assert_eq!(classify(&d, 1, &pct_only), Classification::WithinTolerance);
        let tax = Discrepancy::<u32>::TaxMismatch {
            expected: usd(100),
            invoiced: usd(101),
        };
        assert_eq!(classify(&tax, 0, &tol()), Classification::CreditNote);
    }

    #[test]
    fn quantity_and_rounding_tolerances() {
        let t = tol();
        let over = |ordered, invoiced| Discrepancy::QuantityOverOrdered {
            item: 1u32,
            ordered,
            invoiced,
        };
        assert_eq!(
            classify(&over(100, 102), 0, &t),
            Classification::WithinTolerance
        );
        assert_eq!(classify(&over(100, 103), 0, &t), Classification::CreditNote);
        let both = Tolerance {
            quantity_bps: 100,
            ..t
        };
        // 1% of 100 = 1 unit, stricter than 2.
        assert_eq!(
            classify(&over(100, 102), 0, &both),
            Classification::CreditNote
        );
        let under = Discrepancy::QuantityUnderReceived {
            item: 1u32,
            received: 100,
            invoiced: 90,
        };
        assert_eq!(classify(&under, 0, &t), Classification::Review);
        let tax = |e, i| Discrepancy::<u32>::TaxMismatch {
            expected: yen(e),
            invoiced: yen(i),
        };
        assert_eq!(
            classify(&tax(30_200, 30_201), 0, &t),
            Classification::WithinTolerance
        );
        assert_eq!(
            classify(&tax(30_200, 30_199), 0, &t),
            Classification::WithinTolerance
        );
        assert_eq!(
            classify(&tax(30_200, 30_202), 0, &t),
            Classification::CreditNote
        );
        // Never a matter of tolerance.
        assert_eq!(
            classify(&Discrepancy::UnknownItem { item: 3u32 }, 1, &t),
            Classification::CreditNote
        );
        assert_eq!(
            classify(&Discrepancy::NotInvoiced { item: 3u32 }, 0, &t),
            Classification::Review
        );
        assert_eq!(
            classify(&Discrepancy::<u32>::CurrencyMismatch, 0, &t),
            Classification::Review
        );
        let ship = Discrepancy::<u32>::ShippingMismatch {
            ordered: yen(2000),
            invoiced: yen(2001),
        };
        assert_eq!(classify(&ship, 0, &t), Classification::CreditNote);
    }

    #[test]
    fn stored_discrepancies_round_trip_and_differences() {
        let inv = invoice(vec![(1, 110, 1250), (3, 1, 10)], 2500, 1);
        let found = three_way(&order(), JPY, yen(2000), &[(1, 100), (2, 3)], &inv);
        for d in &found {
            let (expected, actual) = match d {
                Discrepancy::PriceMismatch {
                    ordered, invoiced, ..
                }
                | Discrepancy::ShippingMismatch { ordered, invoiced } => {
                    (Some(ordered.minor()), Some(invoiced.minor()))
                }
                Discrepancy::TaxMismatch { expected, invoiced } => {
                    (Some(expected.minor()), Some(invoiced.minor()))
                }
                Discrepancy::QuantityOverOrdered {
                    ordered: a,
                    invoiced: b,
                    ..
                }
                | Discrepancy::QuantityOverReceived {
                    received: a,
                    invoiced: b,
                    ..
                }
                | Discrepancy::QuantityUnderReceived {
                    received: a,
                    invoiced: b,
                    ..
                } => (Some(i64::from(*a)), Some(i64::from(*b))),
                _ => (None, None),
            };
            let item = match d {
                Discrepancy::UnknownItem { item }
                | Discrepancy::DuplicateLine { item }
                | Discrepancy::NotInvoiced { item }
                | Discrepancy::PriceMismatch { item, .. }
                | Discrepancy::QuantityOverOrdered { item, .. }
                | Discrepancy::QuantityOverReceived { item, .. }
                | Discrepancy::QuantityUnderReceived { item, .. } => *item,
                _ => 0,
            };
            let back = Discrepancy::from_stored(d.code(), item, expected, actual, JPY);
            assert_eq!(back.as_ref(), Some(d));
        }
        assert_eq!(
            Discrepancy::<u32>::from_stored("price_mismatch", 1, None, Some(1), JPY),
            None
        );
        assert_eq!(
            Discrepancy::<u32>::from_stored("nonsense", 1, None, None, JPY),
            None
        );
        assert_eq!(difference(&price(1200, 1250), 110), Some(yen(5500)));
        assert_eq!(difference(&price(1250, 1200), 2), Some(yen(100)));
        assert_eq!(
            difference(&Discrepancy::NotInvoiced { item: 1u32 }, 1),
            None
        );
    }
}
