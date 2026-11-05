//! Supplier compliance documents (L6): document kinds, the qualified-invoice registration
//! number (適格請求書発行事業者登録番号, `T` + 13 digits) and its check digit, and the expiry
//! windows the `supplier_watch` agent uses. Dates are day numbers (days since 1970-01-01)
//! in Japan Standard Time, which has no daylight saving: a day starts at 15:00 UTC.

/// What a compliance document is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DocKind {
    Iso9001,
    Iso14001,
    Iatf16949,
    /// 適格請求書発行事業者登録 (invoice system registration).
    QualifiedInvoiceRegistration,
    /// 反社チェック: the check that the supplier has no ties to anti-social forces.
    AntiSocialCheck,
    Insurance,
    Nda,
    Other,
}

impl DocKind {
    pub const ALL: [DocKind; 8] = [
        DocKind::Iso9001,
        DocKind::Iso14001,
        DocKind::Iatf16949,
        DocKind::QualifiedInvoiceRegistration,
        DocKind::AntiSocialCheck,
        DocKind::Insurance,
        DocKind::Nda,
        DocKind::Other,
    ];

    pub const fn code(self) -> &'static str {
        match self {
            DocKind::Iso9001 => "iso9001",
            DocKind::Iso14001 => "iso14001",
            DocKind::Iatf16949 => "iatf16949",
            DocKind::QualifiedInvoiceRegistration => "qualified_invoice_registration",
            DocKind::AntiSocialCheck => "anti_social_check",
            DocKind::Insurance => "insurance",
            DocKind::Nda => "nda",
            DocKind::Other => "other",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.code() == code)
    }

    /// Whether only the newest document of this kind counts for a supplier (a renewal
    /// replaces the old certificate). `Other` documents each stand alone.
    pub const fn newest_only(self) -> bool {
        !matches!(self, DocKind::Other)
    }
}

/// Check digit of a 法人番号 (corporate number) from its 12-digit base, per the National Tax
/// Agency rule: `9 - (Σ Pn × Qn mod 9)`, where `Pn` is the n-th digit counted from the
/// right of the base and `Qn` is 1 for odd n and 2 for even n. Registration numbers of
/// individual businesses are assigned with the same rule.
pub fn check_digit(base: &[u8; 12]) -> u8 {
    let sum: u32 = base
        .iter()
        .rev()
        .enumerate()
        .map(|(i, d)| u32::from(*d) * if i % 2 == 0 { 1 } else { 2 })
        .sum();
    // sum % 9 is at most 8, so the result is 1..=9.
    9 - (sum % 9) as u8
}

/// Whether `digits` is 13 ASCII digits whose first digit is the check digit of the rest.
pub fn is_valid_corporate_number(digits: &str) -> bool {
    let b = digits.as_bytes();
    if b.len() != 13 || !b.iter().all(u8::is_ascii_digit) {
        return false;
    }
    let mut base = [0u8; 12];
    for (slot, d) in base.iter_mut().zip(&b[1..]) {
        *slot = d - b'0';
    }
    check_digit(&base) == b[0] - b'0'
}

/// A qualified-invoice registration number as stored (`T1234567890123`), or `None` when it
/// is not `T` + 13 digits with a valid check digit. Accepts a lower-case `t`, full-width
/// characters and spaces or hyphens between digit groups, as people copy it from invoices.
pub fn normalize_invoice_registration_number(raw: &str) -> Option<String> {
    let mut chars = raw.trim().chars().map(|c| match c {
        // Full-width ０-９ and Ｔ／ｔ.
        '\u{FF10}'..='\u{FF19}' => char::from_u32(c as u32 - 0xFF10 + u32::from(b'0')).unwrap_or(c),
        '\u{FF34}' | '\u{FF54}' => 'T',
        't' => 'T',
        _ => c,
    });
    if chars.next() != Some('T') {
        return None;
    }
    let mut digits = String::with_capacity(13);
    for c in chars {
        match c {
            '0'..='9' => digits.push(c),
            ' ' | '-' | '\u{3000}' | '\u{2010}' | '\u{2212}' | '\u{FF0D}' => {}
            _ => return None,
        }
    }
    is_valid_corporate_number(&digits).then(|| format!("T{digits}"))
}

const DAY: i64 = 86_400;
/// UTC+9, all year.
pub const JST_OFFSET_SECONDS: i64 = 9 * 3600;

/// The Japan-time calendar day of a Unix timestamp.
pub fn jst_day(unix_seconds: i64) -> i64 {
    (unix_seconds + JST_OFFSET_SECONDS).div_euclid(DAY)
}

/// Day number of a proleptic Gregorian date (days since 1970-01-01).
pub fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The agent warns this many days ahead, and more urgently in the last week.
pub const WARN_DAYS: i64 = 30;
pub const URGENT_DAYS: i64 = 7;

/// Where a document stands on a given day. A document is valid through its expiry date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expiry {
    Valid,
    /// Expires in 8..=30 days.
    Soon {
        days_left: i64,
    },
    /// Expires in 0..=7 days (0 = today is the last valid day).
    Urgent {
        days_left: i64,
    },
    /// The last valid day has passed.
    Expired {
        days_over: i64,
    },
}

impl Expiry {
    /// The window code used in exception params and keys, `None` when nothing is due.
    pub const fn window(self) -> Option<&'static str> {
        match self {
            Expiry::Valid => None,
            Expiry::Soon { .. } => Some("within_30"),
            Expiry::Urgent { .. } => Some("within_7"),
            Expiry::Expired { .. } => Some("expired"),
        }
    }
}

pub fn expiry(expires_day: i64, today: i64) -> Expiry {
    let left = expires_day - today;
    if left < 0 {
        Expiry::Expired { days_over: -left }
    } else if left <= URGENT_DAYS {
        Expiry::Urgent { days_left: left }
    } else if left <= WARN_DAYS {
        Expiry::Soon { days_left: left }
    } else {
        Expiry::Valid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip() {
        for k in DocKind::ALL {
            assert_eq!(DocKind::from_code(k.code()), Some(k));
        }
        assert_eq!(DocKind::from_code("iso"), None);
        assert!(!DocKind::Other.newest_only());
        assert!(DocKind::Iso9001.newest_only());
    }

    #[test]
    fn corporate_number_check_digit_follows_the_nta_rule() {
        // 国税庁's own 法人番号.
        assert!(is_valid_corporate_number("7000012050002"));
        assert_eq!(check_digit(&[0, 0, 0, 0, 1, 2, 0, 5, 0, 0, 0, 2]), 7);
        // Every other leading digit is wrong.
        for d in ['1', '2', '3', '4', '5', '6', '8', '9', '0'] {
            assert!(!is_valid_corporate_number(&format!("{d}000012050002")));
        }
        // A sum divisible by 9 gives check digit 9 (never 0).
        assert_eq!(check_digit(&[0; 12]), 9);
        assert!(is_valid_corporate_number("9000000000000"));
        // Swapping two neighbouring digits is caught.
        assert!(!is_valid_corporate_number("7000010250002"));
        assert!(!is_valid_corporate_number("700001205000"));
        assert!(!is_valid_corporate_number("70000120500021"));
        assert!(!is_valid_corporate_number("700001205000a"));
    }

    #[test]
    fn invoice_registration_numbers_are_t_plus_a_valid_corporate_number() {
        let ok = Some("T7000012050002".to_string());
        assert_eq!(normalize_invoice_registration_number("T7000012050002"), ok);
        assert_eq!(
            normalize_invoice_registration_number(" t7000012050002 "),
            ok
        );
        assert_eq!(
            normalize_invoice_registration_number("T7-0000-1205-0002"),
            ok
        );
        assert_eq!(
            normalize_invoice_registration_number("T 7000 0120 50002"),
            ok
        );
        assert_eq!(
            normalize_invoice_registration_number("Ｔ７００００１２０５０００２"),
            ok
        );
        // Wrong check digit, no T, wrong length, other letters.
        assert_eq!(
            normalize_invoice_registration_number("T8000012050002"),
            None
        );
        assert_eq!(normalize_invoice_registration_number("7000012050002"), None);
        assert_eq!(normalize_invoice_registration_number("T700001205000"), None);
        assert_eq!(
            normalize_invoice_registration_number("T70000120500020"),
            None
        );
        assert_eq!(
            normalize_invoice_registration_number("X7000012050002"),
            None
        );
        assert_eq!(
            normalize_invoice_registration_number("T70000120500O2"),
            None
        );
        assert_eq!(normalize_invoice_registration_number(""), None);
    }

    #[test]
    fn civil_days_match_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(2026, 11, 5), 20_762);
        assert_eq!(
            days_from_civil(2024, 2, 29) + 1,
            days_from_civil(2024, 3, 1)
        );
        assert_eq!(days_from_civil(1969, 12, 31), -1);
    }

    #[test]
    fn the_japan_day_turns_at_15_00_utc() {
        // 2026-11-05T14:59:59Z is 23:59:59 on the 5th in Tokyo; one second later it is the 6th.
        let nov5 = days_from_civil(2026, 11, 5);
        let midnight_jst = nov5 * DAY + 15 * 3600; // 2026-11-05T15:00:00Z
        assert_eq!(jst_day(midnight_jst - 1), nov5);
        assert_eq!(jst_day(midnight_jst), nov5 + 1);
        // 00:00 UTC is already 09:00 the same day in Tokyo.
        assert_eq!(jst_day(nov5 * DAY), nov5);
        assert_eq!(jst_day(-1), 0); // 1970-01-01T08:59:59 JST
    }

    #[test]
    fn expiry_windows_have_inclusive_edges() {
        let t = days_from_civil(2026, 11, 5);
        assert_eq!(expiry(t + 31, t), Expiry::Valid);
        assert_eq!(expiry(t + 30, t), Expiry::Soon { days_left: 30 });
        assert_eq!(expiry(t + 8, t), Expiry::Soon { days_left: 8 });
        assert_eq!(expiry(t + 7, t), Expiry::Urgent { days_left: 7 });
        // Valid through the expiry date itself.
        assert_eq!(expiry(t, t), Expiry::Urgent { days_left: 0 });
        assert_eq!(expiry(t - 1, t), Expiry::Expired { days_over: 1 });
        assert_eq!(expiry(t + 30, t).window(), Some("within_30"));
        assert_eq!(expiry(t, t).window(), Some("within_7"));
        assert_eq!(expiry(t - 5, t).window(), Some("expired"));
        assert_eq!(expiry(t + 60, t).window(), None);

        // A certificate valid through 2026-11-05 is still valid at 23:59:59 JST that day
        // and expired one second later.
        let last = days_from_civil(2026, 11, 5);
        let end_utc = last * DAY + 15 * 3600; // 2026-11-06T00:00:00 JST
        assert_eq!(
            expiry(last, jst_day(end_utc - 1)),
            Expiry::Urgent { days_left: 0 }
        );
        assert_eq!(
            expiry(last, jst_day(end_utc)),
            Expiry::Expired { days_over: 1 }
        );
    }
}
