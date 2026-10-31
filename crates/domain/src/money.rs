//! Money is stored as integer minor units plus an ISO 4217 currency.
//! No floating point anywhere in financial arithmetic.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Currency {
    Jpy,
    Usd,
    Eur,
    Inr,
    Cny,
    Krw,
}

impl Currency {
    /// Number of minor-unit digits (JPY and KRW have none).
    pub const fn minor_digits(self) -> u32 {
        match self {
            Currency::Jpy | Currency::Krw => 0,
            Currency::Usd | Currency::Eur | Currency::Inr | Currency::Cny => 2,
        }
    }

    pub const fn code(self) -> &'static str {
        match self {
            Currency::Jpy => "JPY",
            Currency::Usd => "USD",
            Currency::Eur => "EUR",
            Currency::Inr => "INR",
            Currency::Cny => "CNY",
            Currency::Krw => "KRW",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "JPY" => Some(Currency::Jpy),
            "USD" => Some(Currency::Usd),
            "EUR" => Some(Currency::Eur),
            "INR" => Some(Currency::Inr),
            "CNY" => Some(Currency::Cny),
            "KRW" => Some(Currency::Krw),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoneyError {
    CurrencyMismatch { left: Currency, right: Currency },
    Overflow,
    Negative,
}

impl fmt::Display for MoneyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MoneyError::CurrencyMismatch { left, right } => {
                write!(f, "currency mismatch: {} vs {}", left.code(), right.code())
            }
            MoneyError::Overflow => f.write_str("monetary overflow"),
            MoneyError::Negative => f.write_str("negative amount not allowed here"),
        }
    }
}

impl std::error::Error for MoneyError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Money {
    minor: i64,
    currency: Currency,
}

impl Money {
    pub const fn from_minor(minor: i64, currency: Currency) -> Self {
        Self { minor, currency }
    }

    pub const fn zero(currency: Currency) -> Self {
        Self { minor: 0, currency }
    }

    pub const fn minor(self) -> i64 {
        self.minor
    }

    pub const fn currency(self) -> Currency {
        self.currency
    }

    pub fn checked_add(self, other: Money) -> Result<Money, MoneyError> {
        if self.currency != other.currency {
            return Err(MoneyError::CurrencyMismatch {
                left: self.currency,
                right: other.currency,
            });
        }
        self.minor
            .checked_add(other.minor)
            .map(|minor| Money::from_minor(minor, self.currency))
            .ok_or(MoneyError::Overflow)
    }

    /// Unit price times quantity (quantity in whole units).
    pub fn checked_mul_qty(self, qty: u32) -> Result<Money, MoneyError> {
        self.minor
            .checked_mul(i64::from(qty))
            .map(|minor| Money::from_minor(minor, self.currency))
            .ok_or(MoneyError::Overflow)
    }

    /// Sum of amounts that must all share `currency`. Empty input yields zero.
    pub fn sum(currency: Currency, items: &[Money]) -> Result<Money, MoneyError> {
        items
            .iter()
            .try_fold(Money::zero(currency), |acc, m| acc.checked_add(*m))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpy_has_no_minor_units() {
        assert_eq!(Currency::Jpy.minor_digits(), 0);
        assert_eq!(Currency::Usd.minor_digits(), 2);
    }

    #[test]
    fn landed_total_in_jpy() {
        let unit = Money::from_minor(12_800, Currency::Jpy);
        let lines = unit.checked_mul_qty(25);
        assert_eq!(lines, Ok(Money::from_minor(320_000, Currency::Jpy)));
        let shipping = Money::from_minor(1_500, Currency::Jpy);
        let total = Money::sum(
            Currency::Jpy,
            &[Money::from_minor(320_000, Currency::Jpy), shipping],
        );
        assert_eq!(total, Ok(Money::from_minor(321_500, Currency::Jpy)));
    }

    #[test]
    fn mixing_currencies_is_an_error_not_a_conversion() {
        let a = Money::from_minor(100, Currency::Jpy);
        let b = Money::from_minor(100, Currency::Usd);
        assert_eq!(
            a.checked_add(b),
            Err(MoneyError::CurrencyMismatch {
                left: Currency::Jpy,
                right: Currency::Usd
            })
        );
    }

    #[test]
    fn overflow_is_detected() {
        let big = Money::from_minor(i64::MAX, Currency::Jpy);
        assert_eq!(
            big.checked_add(Money::from_minor(1, Currency::Jpy)),
            Err(MoneyError::Overflow)
        );
        assert_eq!(big.checked_mul_qty(2), Err(MoneyError::Overflow));
    }

    #[test]
    fn currency_codes_round_trip() {
        for c in [
            Currency::Jpy,
            Currency::Usd,
            Currency::Eur,
            Currency::Inr,
            Currency::Cny,
            Currency::Krw,
        ] {
            assert_eq!(Currency::from_code(c.code()), Some(c));
        }
        assert_eq!(Currency::from_code("XXX"), None);
    }
}
