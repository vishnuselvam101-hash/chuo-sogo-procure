//! Consumer shop (Phase E): orders by individuals for published listings, sold by Chuo-Sogo
//! at tax-inclusive prices and paid by bank transfer. Who may move an order between states,
//! and how its totals are computed. Deterministic, no I/O.

use crate::invoicing::{self, InvoiceError, Line, Totals};
use crate::matching::TaxRounding;
use crate::money::{Currency, Money};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderStatus {
    /// Placed by the consumer; Chuo-Sogo checks availability.
    Placed,
    /// Accepted; the invoice with bank details is issued; waiting for payment.
    Confirmed,
    Paid,
    Shipped,
    Completed,
    Cancelled,
}

impl OrderStatus {
    pub const ALL: [OrderStatus; 6] = [
        OrderStatus::Placed,
        OrderStatus::Confirmed,
        OrderStatus::Paid,
        OrderStatus::Shipped,
        OrderStatus::Completed,
        OrderStatus::Cancelled,
    ];

    pub const fn code(self) -> &'static str {
        match self {
            OrderStatus::Placed => "placed",
            OrderStatus::Confirmed => "confirmed",
            OrderStatus::Paid => "paid",
            OrderStatus::Shipped => "shipped",
            OrderStatus::Completed => "completed",
            OrderStatus::Cancelled => "cancelled",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        OrderStatus::ALL.into_iter().find(|s| s.code() == code)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Consumer,
    /// Chuo-Sogo staff with `manage_shop`.
    Shop,
}

pub fn can_move(from: OrderStatus, to: OrderStatus, actor: Actor) -> bool {
    use OrderStatus::*;
    matches!(
        (actor, from, to),
        // The consumer can withdraw an order until it is confirmed.
        (Actor::Consumer, Placed, Cancelled)
            | (Actor::Shop, Placed, Confirmed)
            | (Actor::Shop, Confirmed, Paid)
            | (Actor::Shop, Paid, Shipped)
            | (Actor::Shop, Shipped, Completed)
            // Chuo-Sogo can cancel until the goods leave (a paid order needs a refund outside
            // the platform, recorded in the reason).
            | (Actor::Shop, Placed | Confirmed | Paid, Cancelled)
    )
}

pub const MAX_ORDER_LINES: usize = 50;
pub const MAX_LINE_QUANTITY: u32 = 1_000;

/// Shipping for an order: free from `free_over` (tax-inclusive items total), else `fee`.
pub fn shipping_fee(items_total: Money, fee: Money, free_over: Option<Money>) -> Money {
    match free_over {
        Some(limit) if items_total.minor() >= limit.minor() => Money::zero(fee.currency()),
        _ => fee,
    }
}

/// Order totals: items and shipping (shipping at the standard rate), all tax-inclusive.
pub fn order_totals(
    lines: &[Line],
    shipping: Money,
    currency: Currency,
    rounding: TaxRounding,
) -> Result<Totals, InvoiceError> {
    let mut all = lines.to_vec();
    if shipping.minor() > 0 {
        all.push(Line {
            quantity: 1,
            unit_price: shipping,
            tax_rate_bps: invoicing::STANDARD_RATE_BPS,
        });
    }
    invoicing::totals_inclusive(&all, currency, rounding)
}

#[cfg(test)]
mod tests {
    use super::*;
    use OrderStatus::*;

    fn yen(m: i64) -> Money {
        Money::from_minor(m, Currency::Jpy)
    }

    #[test]
    fn order_lifecycle() {
        assert!(can_move(Placed, Cancelled, Actor::Consumer));
        assert!(
            !can_move(Confirmed, Cancelled, Actor::Consumer),
            "invoice already issued"
        );
        assert!(!can_move(Placed, Confirmed, Actor::Consumer));
        assert!(
            !can_move(Confirmed, Paid, Actor::Consumer),
            "consumers never mark paid"
        );
        for (a, b) in [
            (Placed, Confirmed),
            (Confirmed, Paid),
            (Paid, Shipped),
            (Shipped, Completed),
        ] {
            assert!(can_move(a, b, Actor::Shop));
        }
        assert!(
            !can_move(Placed, Paid, Actor::Shop),
            "no payment before the invoice"
        );
        assert!(!can_move(Shipped, Cancelled, Actor::Shop));
        assert!(!can_move(Cancelled, Placed, Actor::Shop));
    }

    #[test]
    fn shipping_and_totals() {
        assert_eq!(shipping_fee(yen(4999), yen(800), Some(yen(5000))), yen(800));
        assert_eq!(shipping_fee(yen(5000), yen(800), Some(yen(5000))), yen(0));
        assert_eq!(shipping_fee(yen(99_999), yen(800), None), yen(800));
        let lines = [Line {
            quantity: 2,
            unit_price: yen(1100),
            tax_rate_bps: 1000,
        }];
        let t = order_totals(&lines, yen(880), Currency::Jpy, TaxRounding::Floor).unwrap();
        assert_eq!(t.total, yen(3080));
        assert_eq!(t.tax, yen(280));
    }
}
