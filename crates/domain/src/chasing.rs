//! Supplier chasing rules (agents lane L3): when an order needs a follow-up, which attempt it
//! is, and the polite supplier text the follow-up proposes. Pure: no I/O, the caller passes
//! the facts and the current time.
//!
//! Business days are counted in Japan time (JST, UTC+9, no daylight saving): Monday to
//! Friday, except the customary year-end break (29 December to 3 January). Public holidays
//! are not modelled; the cadence is a reminder for a person, not a contract deadline.

use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc, Weekday};

/// What a follow-up asks the supplier for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChaseType {
    /// The order email was sent but the supplier has not acknowledged it.
    OrderAck,
    /// Acknowledged, but no delivery date is known.
    DeliveryDate,
    /// The expected delivery date passed and not everything arrived.
    OverdueDelivery,
    /// Everything arrived but no invoice was recorded.
    MissingInvoice,
}

impl ChaseType {
    pub const ALL: [ChaseType; 4] = [
        ChaseType::OrderAck,
        ChaseType::DeliveryDate,
        ChaseType::OverdueDelivery,
        ChaseType::MissingInvoice,
    ];

    pub const fn code(self) -> &'static str {
        match self {
            ChaseType::OrderAck => "order_ack",
            ChaseType::DeliveryDate => "delivery_date",
            ChaseType::OverdueDelivery => "overdue_delivery",
            ChaseType::MissingInvoice => "missing_invoice",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.code() == code)
    }
}

/// Per-organisation cadence, in JST business days.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cadence {
    /// After the order email, before asking for an acknowledgement.
    pub ack_days: i32,
    /// After the order email, before asking for a delivery date (once acknowledged).
    pub delivery_date_days: i32,
    /// After the expected delivery date, before asking where the goods are.
    pub overdue_days: i32,
    /// After everything arrived, before asking for the invoice.
    pub invoice_days: i32,
    /// Between one follow-up and the next.
    pub repeat_days: i32,
    /// Follow-ups per order and type; after that a person takes over.
    pub max_attempts: i32,
}

impl Default for Cadence {
    fn default() -> Self {
        Cadence {
            ack_days: 2,
            delivery_date_days: 3,
            overdue_days: 1,
            invoice_days: 10,
            repeat_days: 3,
            max_attempts: 3,
        }
    }
}

/// Allowed ranges (the database checks the same).
pub const DAYS_RANGE: std::ops::RangeInclusive<i32> = 1..=30;
pub const INVOICE_DAYS_RANGE: std::ops::RangeInclusive<i32> = 1..=60;
pub const ATTEMPTS_RANGE: std::ops::RangeInclusive<i32> = 1..=10;

impl Cadence {
    /// The name of the first field out of range.
    pub fn validate(&self) -> Result<(), &'static str> {
        let checks = [
            ("ack_days", self.ack_days, &DAYS_RANGE),
            ("delivery_date_days", self.delivery_date_days, &DAYS_RANGE),
            ("overdue_days", self.overdue_days, &DAYS_RANGE),
            ("invoice_days", self.invoice_days, &INVOICE_DAYS_RANGE),
            ("repeat_days", self.repeat_days, &DAYS_RANGE),
            ("max_attempts", self.max_attempts, &ATTEMPTS_RANGE),
        ];
        match checks.into_iter().find(|(_, v, r)| !r.contains(v)) {
            Some((name, _, _)) => Err(name),
            None => Ok(()),
        }
    }
}

/// The calendar date in Japan at `t`.
pub fn jst_date(t: DateTime<Utc>) -> NaiveDate {
    (t + Duration::hours(9)).date_naive()
}

/// Monday to Friday, outside the year-end break (29 Dec – 3 Jan).
pub fn is_business_day(d: NaiveDate) -> bool {
    let weekend = matches!(d.weekday(), Weekday::Sat | Weekday::Sun);
    let year_end = (d.month() == 12 && d.day() >= 29) || (d.month() == 1 && d.day() <= 3);
    !weekend && !year_end
}

/// Business days `d` with `from < d <= to` (0 when `to <= from`).
pub fn business_days_between(from: NaiveDate, to: NaiveDate) -> i64 {
    if to <= from {
        return 0;
    }
    // Bounded: a few years at most in practice; the loop is per day.
    from.iter_days()
        .skip(1)
        .take_while(|d| *d <= to)
        .filter(|d| is_business_day(*d))
        .count() as i64
}

/// What is known about one order.
#[derive(Debug, Clone, Default)]
pub struct OrderFacts {
    /// When the order email was written to the outbox (None: not sent yet).
    pub order_sent_at: Option<DateTime<Utc>>,
    /// The supplier acknowledged the order (recorded, or a confirmed date exists).
    pub acknowledged: bool,
    /// The confirmed delivery date, else the promised one.
    pub expected_on: Option<NaiveDate>,
    /// Waiting for (more) goods: the request is `ordered`.
    pub awaiting_delivery: bool,
    /// Everything arrived (request `delivered`): the last receipt.
    pub received_complete_at: Option<DateTime<Utc>>,
    pub invoiced: bool,
}

/// Follow-ups so far for one order and type.
#[derive(Debug, Clone, Copy, Default)]
pub struct History {
    /// Highest attempt sent or decided.
    pub attempts: i32,
    /// When the last one was sent or decided.
    pub last_at: Option<DateTime<Utc>>,
    /// A proposal is waiting for a person.
    pub pending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    No,
    /// Propose this attempt (1-based).
    Attempt(i32),
    /// All attempts used and the condition still holds: a person takes over.
    Exhausted,
}

/// Where the clock of `t` starts and how many business days it runs, when the condition holds.
fn clock(t: ChaseType, f: &OrderFacts, c: &Cadence) -> Option<(NaiveDate, i32)> {
    match t {
        ChaseType::OrderAck => match f.order_sent_at {
            Some(sent) if f.awaiting_delivery && !f.acknowledged => {
                Some((jst_date(sent), c.ack_days))
            }
            _ => None,
        },
        ChaseType::DeliveryDate => match f.order_sent_at {
            Some(sent) if f.awaiting_delivery && f.acknowledged && f.expected_on.is_none() => {
                Some((jst_date(sent), c.delivery_date_days))
            }
            _ => None,
        },
        ChaseType::OverdueDelivery => match f.expected_on {
            Some(d) if f.awaiting_delivery => Some((d, c.overdue_days)),
            _ => None,
        },
        ChaseType::MissingInvoice => match f.received_complete_at {
            Some(r) if !f.invoiced => Some((jst_date(r), c.invoice_days)),
            _ => None,
        },
    }
}

/// Supplier replies read per order (one supplier) in any 24 hours; more are refused so a
/// flood of messages cannot spend the organisation's model budget.
pub const MAX_REPLY_READINGS_PER_DAY: i64 = 5;

/// Whether one more reply may be read after `recent` readings in the last 24 hours.
pub fn reply_within_daily_limit(recent: i64) -> bool {
    recent < MAX_REPLY_READINGS_PER_DAY
}

/// Whether a follow-up of type `t` is due at `now`.
pub fn due(t: ChaseType, f: &OrderFacts, h: &History, c: &Cadence, now: DateTime<Utc>) -> Due {
    let Some((start, days)) = clock(t, f, c) else {
        return Due::No;
    };
    if h.pending {
        return Due::No;
    }
    let today = jst_date(now);
    if h.attempts <= 0 {
        return if business_days_between(start, today) >= i64::from(days.max(1)) {
            Due::Attempt(1)
        } else {
            Due::No
        };
    }
    let since_last = h
        .last_at
        .map_or(i64::MAX, |l| business_days_between(jst_date(l), today));
    if since_last < i64::from(c.repeat_days.max(1)) {
        Due::No
    } else if h.attempts >= c.max_attempts {
        Due::Exhausted
    } else {
        Due::Attempt(h.attempts + 1)
    }
}

/// Supplier text language: Japanese for suppliers in Japan, English otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Ja,
    En,
}

impl Lang {
    pub fn for_country(country: &str) -> Self {
        if country.eq_ignore_ascii_case("JP") {
            Lang::Ja
        } else {
            Lang::En
        }
    }

    pub const fn code(self) -> &'static str {
        match self {
            Lang::Ja => "ja",
            Lang::En => "en",
        }
    }
}

/// What the follow-up text may refer to. Never the customer: the supplier is reminded of
/// the order by its numbers only.
#[derive(Debug, Clone)]
pub struct DraftContext<'a> {
    pub supplier_name: &'a str,
    pub po_number: &'a str,
    pub rfq_number: &'a str,
    pub expected_on: Option<NaiveDate>,
}

/// A proposed follow-up email (the reply token is added to the subject when it is queued).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub lang: Lang,
    pub subject: String,
    pub body: String,
}

fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .chars()
        .take(200)
        .collect()
}

/// The follow-up text for attempt `attempt` (server-side templates; ja is primary).
pub fn draft(t: ChaseType, attempt: i32, lang: Lang, cx: &DraftContext<'_>) -> Draft {
    let supplier = one_line(cx.supplier_name);
    let po = one_line(cx.po_number);
    let rfq = one_line(cx.rfq_number);
    let date = cx.expected_on.map(|d| d.to_string()).unwrap_or_default();
    let again = attempt > 1;
    match lang {
        Lang::Ja => {
            let (subject, ask) = match t {
                ChaseType::OrderAck => (
                    format!("【ご確認のお願い】注文書 {po} のご受領について"),
                    "先日お送りいたしました下記ご注文につきまして、ご受領のご連絡をまだ頂戴しておりませんでした。\n\
                     お手数をおかけいたしますが、ご受領の旨と納期を本メールへのご返信にてお知らせいただけますでしょうか。"
                        .to_string(),
                ),
                ChaseType::DeliveryDate => (
                    format!("【納期のご確認】注文書 {po}"),
                    "下記ご注文につきまして、納期のご回答をまだ頂戴しておりませんでした。\n\
                     お忙しいところ恐縮ですが、納品予定日を本メールへのご返信にてお知らせいただけますでしょうか。"
                        .to_string(),
                ),
                ChaseType::OverdueDelivery => (
                    format!("【納品状況のご確認】注文書 {po}"),
                    format!(
                        "下記ご注文につきまして、納品予定日（{date}）を過ぎておりますが、まだ全数の納品を確認できておりません。\n\
                         恐れ入りますが、出荷状況と新たな納品予定日をお知らせいただけますでしょうか。"
                    ),
                ),
                ChaseType::MissingInvoice => (
                    format!("【ご請求書のお願い】注文書 {po}"),
                    "下記ご注文の品物はすべて受領いたしました。誠にありがとうございました。\n\
                     つきましては、ご注文時のご案内のとおり、注文番号を記載のうえご請求書をお送りいただけますでしょうか。"
                        .to_string(),
                ),
            };
            let opening = if again {
                "度々のご連絡となり誠に恐縮ですが、改めてご確認をお願い申し上げます。\n\n"
            } else {
                ""
            };
            let body = format!(
                "{supplier} 御中\n\n\
                 平素より大変お世話になっております。Chuo-Sogo 購買担当でございます。\n\n\
                 {opening}{ask}\n\n\
                 注文番号：{po}\n\
                 お見積番号：{rfq}\n\n\
                 本メールと行き違いでご連絡をいただいておりましたら、何卒ご容赦ください。\n\
                 件名を変えずにご返信いただけますと幸いです。\n\n\
                 引き続きどうぞよろしくお願い申し上げます。\n\
                 Chuo-Sogo 購買担当"
            );
            Draft {
                lang,
                subject,
                body,
            }
        }
        Lang::En => {
            let (subject, ask) = match t {
                ChaseType::OrderAck => (
                    format!("Follow-up: acknowledgement of purchase order {po}"),
                    "We have not yet received your acknowledgement of the order below.\n\
                     Could you please confirm that you received it, and let us know the delivery date, by replying to this email?"
                        .to_string(),
                ),
                ChaseType::DeliveryDate => (
                    format!("Delivery date for purchase order {po}"),
                    "We have not yet received a delivery date for the order below.\n\
                     Could you please let us know when you expect to deliver it?"
                        .to_string(),
                ),
                ChaseType::OverdueDelivery => (
                    format!("Delivery status of purchase order {po}"),
                    format!(
                        "The expected delivery date of the order below ({date}) has passed and we have not yet received everything.\n\
                         Could you please let us know the shipping status and the new delivery date?"
                    ),
                ),
                ChaseType::MissingInvoice => (
                    format!("Invoice for purchase order {po}"),
                    "Thank you: everything on the order below has arrived.\n\
                     Could you please send your invoice, quoting the purchase order number, as described in the order?"
                        .to_string(),
                ),
            };
            let opening = if again {
                "We are sorry to write again about this.\n\n"
            } else {
                ""
            };
            let body = format!(
                "Dear {supplier},\n\n\
                 {opening}{ask}\n\n\
                 Purchase order number: {po}\n\
                 Quotation reference: {rfq}\n\n\
                 If you have already replied, please disregard this message.\n\
                 Please reply without changing the subject.\n\n\
                 Kind regards,\n\
                 Chuo-Sogo purchasing"
            );
            Draft {
                lang,
                subject,
                body,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap_or_default()
    }

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s)
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc.timestamp_opt(0, 0).unwrap())
    }

    #[test]
    fn business_days_skip_weekends_and_the_year_end_break() {
        // 2026-11-06 is a Friday.
        assert_eq!(business_days_between(d("2026-11-06"), d("2026-11-09")), 1);
        assert_eq!(business_days_between(d("2026-11-06"), d("2026-11-08")), 0);
        assert_eq!(business_days_between(d("2026-11-09"), d("2026-11-09")), 0);
        assert_eq!(business_days_between(d("2026-11-10"), d("2026-11-09")), 0);
        assert_eq!(business_days_between(d("2026-11-02"), d("2026-11-13")), 9);
        // 28 Dec 2026 (Mon) to 5 Jan 2027 (Tue): 29 Dec – 3 Jan are off; 4 and 5 Jan count.
        assert_eq!(business_days_between(d("2026-12-28"), d("2027-01-05")), 2);
        assert!(!is_business_day(d("2026-12-30")));
        assert!(is_business_day(d("2026-12-28")));
    }

    #[test]
    fn dates_are_japan_dates() {
        // 14:59 UTC is 23:59 JST the same day; 15:00 UTC is already the next day.
        assert_eq!(jst_date(utc("2026-11-06T14:59:00Z")), d("2026-11-06"));
        assert_eq!(jst_date(utc("2026-11-06T15:00:00Z")), d("2026-11-07"));
    }

    fn sent(at: &str) -> OrderFacts {
        OrderFacts {
            order_sent_at: Some(utc(at)),
            awaiting_delivery: true,
            ..Default::default()
        }
    }

    #[test]
    fn acknowledgement_is_asked_after_n_business_days_in_jst() {
        let c = Cadence::default(); // 2 days
        let h = History::default();
        // Sent Thursday 5 Nov 23:30 JST (14:30 UTC): Fri 6 and Mon 9 are the two days.
        let f = sent("2026-11-05T14:30:00Z");
        let t = ChaseType::OrderAck;
        assert_eq!(due(t, &f, &h, &c, utc("2026-11-08T14:00:00Z")), Due::No); // Sun JST
        assert_eq!(due(t, &f, &h, &c, utc("2026-11-08T14:59:00Z")), Due::No); // Sun 23:59 JST
        assert_eq!(
            due(t, &f, &h, &c, utc("2026-11-08T15:00:00Z")), // Mon 00:00 JST
            Due::Attempt(1)
        );
        // Sent Thursday 5 Nov 00:30 JST = Wed 4 Nov 15:30 UTC: the clock starts on Thursday.
        let f = sent("2026-11-04T15:30:00Z");
        assert_eq!(
            due(t, &f, &h, &c, utc("2026-11-06T01:00:00Z")),
            Due::No // Fri 10:00 JST: one day
        );
        let mut acked = f.clone();
        acked.acknowledged = true;
        assert_eq!(due(t, &acked, &h, &c, utc("2026-12-01T00:00:00Z")), Due::No);
        let mut unsent = f.clone();
        unsent.order_sent_at = None;
        assert_eq!(
            due(t, &unsent, &h, &c, utc("2026-12-01T00:00:00Z")),
            Due::No
        );
    }

    #[test]
    fn repeats_wait_and_stop_after_the_last_attempt() {
        let c = Cadence::default(); // repeat 3, max 3
        let f = sent("2026-11-02T00:00:00Z");
        let t = ChaseType::OrderAck;
        let mut h = History {
            attempts: 1,
            last_at: Some(utc("2026-11-09T01:00:00Z")), // Mon
            pending: false,
        };
        assert_eq!(due(t, &f, &h, &c, utc("2026-11-11T01:00:00Z")), Due::No); // Wed: 2
        assert_eq!(
            due(t, &f, &h, &c, utc("2026-11-12T01:00:00Z")), // Thu: 3
            Due::Attempt(2)
        );
        h.pending = true;
        assert_eq!(due(t, &f, &h, &c, utc("2026-11-30T01:00:00Z")), Due::No);
        h.pending = false;
        h.attempts = 3;
        assert_eq!(
            due(t, &f, &h, &c, utc("2026-11-30T01:00:00Z")),
            Due::Exhausted
        );
    }

    #[test]
    fn each_condition_has_its_own_clock() {
        let c = Cadence::default();
        let h = History::default();
        let now = utc("2026-11-20T03:00:00Z"); // Fri
        // No delivery date once acknowledged.
        let mut f = sent("2026-11-10T00:00:00Z");
        f.acknowledged = true;
        assert_eq!(
            due(ChaseType::DeliveryDate, &f, &h, &c, now),
            Due::Attempt(1)
        );
        assert_eq!(due(ChaseType::OrderAck, &f, &h, &c, now), Due::No);
        f.expected_on = Some(d("2026-11-19"));
        assert_eq!(due(ChaseType::DeliveryDate, &f, &h, &c, now), Due::No);
        // Overdue: the day after the expected date (one business day).
        assert_eq!(
            due(ChaseType::OverdueDelivery, &f, &h, &c, now),
            Due::Attempt(1)
        );
        f.expected_on = Some(d("2026-11-20"));
        assert_eq!(due(ChaseType::OverdueDelivery, &f, &h, &c, now), Due::No);
        f.awaiting_delivery = false;
        f.expected_on = Some(d("2026-11-01"));
        assert_eq!(due(ChaseType::OverdueDelivery, &f, &h, &c, now), Due::No);
        // Missing invoice: 10 business days after the last receipt.
        f.received_complete_at = Some(utc("2026-11-06T01:00:00Z"));
        assert_eq!(
            due(ChaseType::MissingInvoice, &f, &h, &c, now),
            Due::Attempt(1)
        );
        f.received_complete_at = Some(utc("2026-11-09T01:00:00Z"));
        assert_eq!(due(ChaseType::MissingInvoice, &f, &h, &c, now), Due::No);
        f.received_complete_at = Some(utc("2026-11-06T01:00:00Z"));
        f.invoiced = true;
        assert_eq!(due(ChaseType::MissingInvoice, &f, &h, &c, now), Due::No);
    }

    #[test]
    fn cadence_is_validated() {
        assert_eq!(Cadence::default().validate(), Ok(()));
        let bad = Cadence {
            repeat_days: 0,
            ..Cadence::default()
        };
        assert_eq!(bad.validate(), Err("repeat_days"));
        let bad = Cadence {
            invoice_days: 61,
            ..Cadence::default()
        };
        assert_eq!(bad.validate(), Err("invoice_days"));
    }

    #[test]
    fn drafts_are_polite_bilingual_by_country_and_name_only_order_numbers() {
        let cx = DraftContext {
            supplier_name: "Tokyo Bearings KK",
            po_number: "PO-77",
            rfq_number: "RFQ-2026-000001",
            expected_on: Some(d("2026-11-19")),
        };
        assert_eq!(Lang::for_country("jp"), Lang::Ja);
        assert_eq!(Lang::for_country("DE"), Lang::En);
        for t in ChaseType::ALL {
            let ja = draft(t, 1, Lang::Ja, &cx);
            assert!(ja.body.starts_with("Tokyo Bearings KK 御中"));
            assert!(ja.body.contains("お世話になっております"));
            assert!(ja.body.contains("PO-77") && ja.body.contains("RFQ-2026-000001"));
            assert!(ja.subject.contains("PO-77"));
            assert!(!ja.body.contains("度々"));
            let again = draft(t, 2, Lang::Ja, &cx);
            assert!(again.body.contains("度々のご連絡"));
            let en = draft(t, 1, Lang::En, &cx);
            assert!(en.body.starts_with("Dear Tokyo Bearings KK,"));
            assert!(en.body.is_ascii());
        }
        assert!(
            draft(ChaseType::OverdueDelivery, 1, Lang::Ja, &cx)
                .body
                .contains("2026-11-19")
        );
        // Control characters in names cannot break the text into new lines.
        let evil = DraftContext {
            supplier_name: "A\r\nBcc: x@y",
            ..cx
        };
        assert!(
            !draft(ChaseType::OrderAck, 1, Lang::En, &evil)
                .body
                .contains("\r")
        );
    }

    #[test]
    fn chase_type_codes_round_trip() {
        for t in ChaseType::ALL {
            assert_eq!(ChaseType::from_code(t.code()), Some(t));
        }
        assert_eq!(ChaseType::from_code("other"), None);
    }
}
