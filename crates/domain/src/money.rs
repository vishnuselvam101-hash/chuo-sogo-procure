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

    /// Parses a plain non-negative decimal amount ("1234", "1,234.50") exactly into minor
    /// units. Commas are accepted only as well-formed thousands separators ("12,50" or "1,2"
    /// are refused, never read as 1250 or 12). More fraction digits than the currency has,
    /// signs, exponents or any other characters are refused rather than rounded.
    pub fn parse_decimal(text: &str, currency: Currency) -> Option<Money> {
        let t = text.trim();
        let (whole_raw, frac) = match t.split_once('.') {
            Some((w, f)) => (w, f),
            None => (t, ""),
        };
        if whole_raw.contains(',') {
            let mut groups = whole_raw.split(',');
            let first = groups.next().unwrap_or("");
            let well_formed = (1..=3).contains(&first.len()) && groups.all(|g| g.len() == 3);
            if !well_formed {
                return None;
            }
        }
        let whole_owned: String = whole_raw.chars().filter(|c| *c != ',').collect();
        let whole = whole_owned.as_str();
        let digits = currency.minor_digits() as usize;
        if whole.is_empty()
            || !whole.bytes().all(|b| b.is_ascii_digit())
            || !frac.bytes().all(|b| b.is_ascii_digit())
            || frac.len() > digits
        {
            return None;
        }
        let scale = 10_i64.checked_pow(currency.minor_digits())?;
        let whole: i64 = whole.parse().ok()?;
        let frac: i64 = if digits == 0 {
            0
        } else {
            format!("{frac:0<digits$}").parse().ok()?
        };
        whole
            .checked_mul(scale)?
            .checked_add(frac)
            .map(|minor| Money::from_minor(minor, currency))
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
    fn decimal_parsing_is_exact_and_strict() {
        let p = Money::parse_decimal;
        assert_eq!(
            p("1,234", Currency::Jpy),
            Some(Money::from_minor(1234, Currency::Jpy))
        );
        assert_eq!(
            p("12.5", Currency::Usd),
            Some(Money::from_minor(1250, Currency::Usd))
        );
        assert_eq!(
            p("0.07", Currency::Usd),
            Some(Money::from_minor(7, Currency::Usd))
        );
        assert_eq!(p("12.5", Currency::Jpy), None);
        assert_eq!(p("1.234", Currency::Usd), None);
        assert_eq!(
            p("1,234,567.5", Currency::Usd),
            Some(Money::from_minor(123_456_750, Currency::Usd))
        );
        // Decimal commas and malformed groups are refused, never misread.
        for bad in ["12,50", "1,2", ",5", "1,23,456", "1234,567", "1,234.5,0"] {
            assert_eq!(p(bad, Currency::Eur), None, "{bad}");
        }
        for bad in [
            "",
            "-5",
            "1e3",
            "abc",
            ".5",
            "1.2.3",
            "¥100",
            "99999999999999999999",
        ] {
            assert_eq!(p(bad, Currency::Usd), None, "{bad}");
        }
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
