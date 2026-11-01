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
}
