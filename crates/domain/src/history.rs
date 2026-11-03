//! A customer's past purchases, imported from the spreadsheets and ERP exports they already
//! keep (Excel or CSV, Japanese or English headers). This is the "decision material" the
//! service needs from day one: what was bought, from whom, at what price, how long ago.
//!
//! Everything here is deterministic. Prices are kept exactly, in thousandths of a minor unit
//! (sub-yen unit prices for screws and resistors are common); a row that cannot be read
//! exactly is rejected with a reason, never guessed.

use crate::money::Currency;

/// Columns the importer understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    Date,
    Supplier,
    Description,
    Manufacturer,
    Mpn,
    Quantity,
    Unit,
    UnitPrice,
    LineTotal,
    Currency,
    LeadTimeDays,
    Notes,
}

impl Field {
    pub const ALL: [Field; 12] = [
        Field::Date,
        Field::Supplier,
        Field::Description,
        Field::Manufacturer,
        Field::Mpn,
        Field::Quantity,
        Field::Unit,
        Field::UnitPrice,
        Field::LineTotal,
        Field::Currency,
        Field::LeadTimeDays,
        Field::Notes,
    ];

    pub const fn code(self) -> &'static str {
        match self {
            Field::Date => "date",
            Field::Supplier => "supplier",
            Field::Description => "description",
            Field::Manufacturer => "manufacturer",
            Field::Mpn => "mpn",
            Field::Quantity => "quantity",
            Field::Unit => "unit",
            Field::UnitPrice => "unit_price",
            Field::LineTotal => "line_total",
            Field::Currency => "currency",
            Field::LeadTimeDays => "lead_time_days",
            Field::Notes => "notes",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Field::ALL.into_iter().find(|f| f.code() == code)
    }

    /// Header names (already normalised by `header_key`) recognised for this field.
    const fn synonyms(self) -> &'static [&'static str] {
        match self {
            Field::Date => &[
                "日付",
                "購入日",
                "発注日",
                "注文日",
                "納品日",
                "仕入日",
                "仕入日付",
                "伝票日付",
                "年月日",
                "検収日",
                "date",
                "orderdate",
                "purchasedate",
                "podate",
            ],
            Field::Supplier => &[
                "仕入先",
                "仕入先名",
                "購入先",
                "購入先名",
                "発注先",
                "発注先名",
                "取引先",
                "取引先名",
                "業者",
                "業者名",
                "商社",
                "supplier",
                "suppliername",
                "vendor",
                "vendorname",
            ],
            Field::Description => &[
                "品名",
                "品目",
                "品目名",
                "商品名",
                "製品名",
                "部品名",
                "名称",
                "摘要",
                "item",
                "itemname",
                "description",
                "product",
                "productname",
            ],
            Field::Manufacturer => &[
                "メーカー",
                "メーカー名",
                "製造元",
                "maker",
                "manufacturer",
                "brand",
            ],
            Field::Mpn => &[
                "型番",
                "品番",
                "型式",
                "部品番号",
                "製品番号",
                "メーカー型番",
                "mpn",
                "partnumber",
                "partno",
                "model",
                "modelnumber",
            ],
            Field::Quantity => &[
                "数量",
                "個数",
                "発注数",
                "発注数量",
                "購入数",
                "購入数量",
                "qty",
                "quantity",
            ],
            Field::Unit => &["単位", "unit", "uom"],
            Field::UnitPrice => &[
                "単価",
                "仕入単価",
                "購入単価",
                "発注単価",
                "unitprice",
                "price",
            ],
            Field::LineTotal => &[
                "金額",
                "合計",
                "小計",
                "合計金額",
                "税抜金額",
                "仕入金額",
                "amount",
                "total",
                "linetotal",
                "subtotal",
            ],
            Field::Currency => &["通貨", "currency"],
            Field::LeadTimeDays => &["納期日数", "リードタイム", "leadtime", "leadtimedays"],
            Field::Notes => &["備考", "メモ", "notes", "remarks", "memo"],
        }
    }
}

/// Full-width ASCII (ＡＢＣ１２３) and the ideographic space become their half-width forms,
/// as Japanese spreadsheets mix both.
pub fn half_width(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{3000}' => ' ',
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            _ => c,
        })
        .collect()
}

/// Header text without case, spaces, punctuation or a bracketed suffix ("単価（円）" → "単価").
pub(crate) fn header_key(raw: &str) -> String {
    let s = half_width(raw).to_lowercase();
    let mut out = String::new();
    let mut depth = 0u32;
    for c in s.chars() {
        match c {
            '(' | '[' | '（' | '【' | '「' => depth += 1,
            ')' | ']' | '）' | '】' | '」' => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            _ if c.is_alphanumeric() => out.push(c),
            _ => {}
        }
    }
    out
}

/// Which field each column holds (`None` = ignored). A field is taken by its first column.
pub fn detect_columns(header: &[String]) -> Vec<Option<Field>> {
    let mut taken: Vec<Field> = Vec::new();
    header
        .iter()
        .map(|h| {
            let key = header_key(h);
            let field = Field::ALL
                .into_iter()
                .find(|f| f.synonyms().contains(&key.as_str()))?;
            if taken.contains(&field) {
                return None;
            }
            taken.push(field);
            Some(field)
        })
        .collect()
}

/// The header is the first of the leading rows that names an item (description or part
/// number) plus at least two other known columns; exports often start with a title block.
pub fn find_header(rows: &[Vec<String>]) -> Option<usize> {
    rows.iter().take(15).position(|row| {
        let cols = detect_columns(row);
        let names_item = cols
            .iter()
            .any(|c| matches!(c, Some(Field::Description | Field::Mpn)));
        names_item && cols.iter().flatten().count() >= 3
    })
}

/// Why a row was not imported. Codes are shown to the person importing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowError {
    DateMissing,
    DateInvalid,
    ItemMissing,
    QuantityInvalid,
    PriceMissing,
    PriceInvalid,
    CurrencyInvalid,
    LeadTimeInvalid,
    TextTooLong,
}

impl RowError {
    pub const fn code(self) -> &'static str {
        match self {
            RowError::DateMissing => "date_missing",
            RowError::DateInvalid => "date_invalid",
            RowError::ItemMissing => "item_missing",
            RowError::QuantityInvalid => "quantity_invalid",
            RowError::PriceMissing => "price_missing",
            RowError::PriceInvalid => "price_invalid",
            RowError::CurrencyInvalid => "currency_invalid",
            RowError::LeadTimeInvalid => "lead_time_invalid",
            RowError::TextTooLong => "text_too_long",
        }
    }
}

/// A calendar date (validated).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Day {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl Day {
    pub fn new(year: i32, month: u32, day: u32) -> Option<Self> {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let days = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => return None,
        };
        ((1990..=2100).contains(&year) && (1..=days).contains(&day)).then_some(Day {
            year,
            month,
            day,
        })
    }

    pub fn iso(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// Days since 1970-01-01 (civil calendar).
    pub fn epoch_days(self) -> i64 {
        let (y, m) = if self.month <= 2 {
            (i64::from(self.year) - 1, i64::from(self.month) + 9)
        } else {
            (i64::from(self.year), i64::from(self.month) - 3)
        };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * m + 2) / 5 + i64::from(self.day) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    fn from_epoch_days(z: i64) -> Option<Self> {
        let z = z + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).ok()?;
        let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).ok()?;
        let year = i32::try_from(yoe + era * 400 + i64::from(month <= 2)).ok()?;
        Day::new(year, month, day)
    }
}

fn numbers(s: &str) -> Vec<i64> {
    s.split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse().ok())
        .collect()
}

/// `2024/1/5`, `2024-01-05`, `2024.1.5`, `2024年1月5日`, Japanese eras (`令和6年1月5日`,
/// `R6.1.5`, `H31/4/30`), ISO date-times, and Excel serial day numbers (1900 system).
pub fn parse_date(raw: &str) -> Option<Day> {
    let s = half_width(raw.trim());
    if s.is_empty() {
        return None;
    }
    let lower = s.to_lowercase();
    let era_base = [("令和", 2018), ("平成", 1988), ("r", 2018), ("h", 1988)]
        .into_iter()
        .find(|(p, _)| lower.starts_with(p));
    if let Some((_, base)) = era_base {
        // `令和元年5月1日`: the first year of an era is written 元 and has no digit.
        let (y, m, d) = match (s.contains('元'), &numbers(&s)[..]) {
            (true, &[m, d]) => (1, m, d),
            (false, &[y, m, d]) => (y, m, d),
            _ => return None,
        };
        return Day::new(
            base + i32::try_from(y).ok()?,
            u32::try_from(m).ok()?,
            u32::try_from(d).ok()?,
        );
    }
    // Excel serial: 1990-01-01 is 32874, 2100-12-31 is 73415; an optional time part.
    let serial = s.split_once('.').map_or(s.as_str(), |(w, f)| {
        if f.bytes().all(|b| b.is_ascii_digit()) {
            w
        } else {
            ""
        }
    });
    if serial.len() == 5 && serial.bytes().all(|b| b.is_ascii_digit()) {
        let v: i64 = serial.parse().ok()?;
        // The 1900 system counts a non-existent 1900-02-29; every date after it is one higher.
        return Day::from_epoch_days(v - 25_569);
    }
    let date_part = s.split(['T', ' ']).next().unwrap_or("");
    let &[y, m, d] = &numbers(date_part)[..] else {
        return None;
    };
    if !(1000..=9999).contains(&y) {
        return None;
    }
    Day::new(
        i32::try_from(y).ok()?,
        u32::try_from(m).ok()?,
        u32::try_from(d).ok()?,
    )
}

/// A non-negative decimal as an integer scaled by 10^`scale`; `None` if it needs more digits
/// than that, or is not a number. Grouping commas, currency signs and a trailing unit word
/// ("円", "個", "pcs") are allowed.
fn parse_scaled(raw: &str, scale: u32) -> Option<i64> {
    let s = half_width(raw.trim());
    let s = s
        .trim_start_matches(['¥', '￥', '$', '€', '₹', '\\'])
        .trim();
    let end = s
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_digit() || *c == ',' || *c == '.'))
        .map_or(s.len(), |(i, _)| i);
    let (num, rest) = s.split_at(end);
    let rest = rest.trim();
    if num.is_empty() || rest.chars().any(|c| c.is_ascii_digit() || c == '-') {
        return None;
    }
    let num: String = num.chars().filter(|c| *c != ',').collect();
    let (whole, frac) = num.split_once('.').unwrap_or((&num, ""));
    if whole.is_empty() || frac.contains('.') || frac.len() > scale as usize {
        return None;
    }
    let whole: i64 = whole.parse().ok()?;
    let frac: i64 = if frac.is_empty() {
        0
    } else {
        format!("{frac:0<w$}", w = scale as usize).parse().ok()?
    };
    whole
        .checked_mul(10_i64.checked_pow(scale)?)?
        .checked_add(frac)
}

fn currency_of(raw: &str) -> Option<Currency> {
    let s = half_width(raw.trim()).to_uppercase();
    match s.as_str() {
        "円" | "¥" | "￥" => Some(Currency::Jpy),
        "$" => Some(Currency::Usd),
        "€" => Some(Currency::Eur),
        "₹" => Some(Currency::Inr),
        "元" => Some(Currency::Cny),
        "원" => Some(Currency::Krw),
        _ => Currency::from_code(&s),
    }
}

/// One past purchase line, ready to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRow {
    pub purchased_on: Day,
    pub supplier: String,
    pub description: String,
    pub manufacturer: String,
    pub mpn: String,
    /// Thousandths of a unit (2.5 m = 2500).
    pub quantity_milli: i64,
    pub unit: String,
    /// Thousandths of a minor unit (¥0.85 = 850).
    pub unit_price_milli: i64,
    /// True when the unit price was worked out from the line total and quantity.
    pub price_derived: bool,
    pub currency: Currency,
    pub lead_time_days: Option<i32>,
    pub notes: String,
}

pub const MAX_QUANTITY_MILLI: i64 = 1_000_000_000_000;
pub const MAX_PRICE_MILLI: i64 = 1_000_000_000_000_000;

fn text(v: &str, max: usize) -> Result<String, RowError> {
    let t = v
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>();
    if t.chars().count() > max {
        return Err(RowError::TextTooLong);
    }
    Ok(t)
}

/// True when every cell is blank (such rows are skipped, not reported).
pub fn is_blank(cells: &[String]) -> bool {
    cells.iter().all(|c| c.trim().is_empty())
}

fn cell<'a>(cells: &'a [String], mapping: &[Option<Field>], f: Field) -> &'a str {
    mapping
        .iter()
        .position(|m| *m == Some(f))
        .and_then(|i| cells.get(i))
        .map_or("", |s| s.trim())
}

/// Reads one data row with the given column mapping. `default_currency` applies when the
/// file has no currency column or the cell is blank.
pub fn parse_row(
    cells: &[String],
    mapping: &[Option<Field>],
    default_currency: Currency,
) -> Result<HistoryRow, RowError> {
    let get = |f: Field| cell(cells, mapping, f);
    let date = get(Field::Date);
    if date.is_empty() {
        return Err(RowError::DateMissing);
    }
    let purchased_on = parse_date(date).ok_or(RowError::DateInvalid)?;
    let description = text(get(Field::Description), 1000)?;
    let mpn = text(&half_width(get(Field::Mpn)), 100)?;
    if description.is_empty() && mpn.is_empty() {
        return Err(RowError::ItemMissing);
    }
    let currency = match get(Field::Currency) {
        "" => default_currency,
        c => currency_of(c).ok_or(RowError::CurrencyInvalid)?,
    };
    let quantity_milli = match get(Field::Quantity) {
        "" => 1000,
        q => parse_scaled(q, 3)
            .filter(|q| (1..=MAX_QUANTITY_MILLI).contains(q))
            .ok_or(RowError::QuantityInvalid)?,
    };
    let price_scale = currency.minor_digits() + 3;
    let (unit_price_milli, price_derived) = match (get(Field::UnitPrice), get(Field::LineTotal)) {
        ("", "") => return Err(RowError::PriceMissing),
        ("", total) => {
            let total = parse_scaled(total, price_scale).ok_or(RowError::PriceInvalid)?;
            // total (milli-minor) / quantity (milli-units), rounded half up to 0.001 minor.
            let num = i128::from(total) * 1000;
            let den = i128::from(quantity_milli);
            let q = (num * 2 + den) / (den * 2);
            (i64::try_from(q).map_err(|_| RowError::PriceInvalid)?, true)
        }
        (price, _) => (
            parse_scaled(price, price_scale).ok_or(RowError::PriceInvalid)?,
            false,
        ),
    };
    if unit_price_milli > MAX_PRICE_MILLI {
        return Err(RowError::PriceInvalid);
    }
    let lead_time_days = match get(Field::LeadTimeDays) {
        "" => None,
        d => Some(
            parse_scaled(d, 0)
                .and_then(|d| i32::try_from(d).ok())
                .filter(|d| *d <= 3650)
                .ok_or(RowError::LeadTimeInvalid)?,
        ),
    };
    Ok(HistoryRow {
        purchased_on,
        supplier: text(get(Field::Supplier), 200)?,
        description,
        manufacturer: text(get(Field::Manufacturer), 200)?,
        mpn,
        quantity_milli,
        unit: text(get(Field::Unit), 16)?,
        unit_price_milli,
        price_derived,
        currency,
        lead_time_days,
        notes: text(get(Field::Notes), 1000)?,
    })
}

/// What a past purchase and a new request line are matched on: the part number when there is
/// one (letters and digits only, upper case: `ＡＢＣ-123 x` = `ABC123X`), else the
/// description (half-width, lower case, single spaces).
pub fn item_key(mpn: &str, description: &str) -> Option<String> {
    let m: String = half_width(mpn)
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect();
    if !m.is_empty() {
        return Some(format!("m:{m}"));
    }
    let d = half_width(description)
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!d.is_empty()).then(|| format!("d:{d}"))
}

/// One past price for the summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricePoint {
    pub purchased_on: Day,
    pub supplier: String,
    pub unit_price_milli: i64,
    pub currency: Currency,
}

/// The price reference shown next to a request line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceSummary {
    pub purchases: usize,
    pub last: PricePoint,
    /// Lowest and highest unit price in the currency of the last purchase.
    pub min_price_milli: i64,
    pub max_price_milli: i64,
    /// Where the lowest price (in that currency) was paid most recently.
    pub cheapest: PricePoint,
    pub suppliers: usize,
}

/// Summary of past purchases of one item. Ties on date keep the input order; the result is
/// the same for the same input.
pub fn summarise(points: &[PricePoint]) -> Option<PriceSummary> {
    let last = points
        .iter()
        .max_by(|a, b| a.purchased_on.cmp(&b.purchased_on))?;
    let same: Vec<&PricePoint> = points
        .iter()
        .filter(|p| p.currency == last.currency)
        .collect();
    let min = same.iter().map(|p| p.unit_price_milli).min()?;
    let max = same.iter().map(|p| p.unit_price_milli).max()?;
    let cheapest = same
        .iter()
        .filter(|p| p.unit_price_milli == min)
        .max_by(|a, b| a.purchased_on.cmp(&b.purchased_on))?;
    let mut suppliers: Vec<&str> = points
        .iter()
        .map(|p| p.supplier.as_str())
        .filter(|s| !s.is_empty())
        .collect();
    suppliers.sort_unstable();
    suppliers.dedup();
    Some(PriceSummary {
        purchases: points.len(),
        last: last.clone(),
        min_price_milli: min,
        max_price_milli: max,
        cheapest: (*cheapest).clone(),
        suppliers: suppliers.len(),
    })
}

/// How a quoted unit price compares with the last price paid, in basis points
/// (+1250 = 12.5 % dearer). `None` across currencies.
pub fn change_bps(
    last_milli: i64,
    last: Currency,
    quoted_minor: i64,
    quoted: Currency,
) -> Option<i64> {
    if last != quoted || last_milli <= 0 {
        return None;
    }
    let quoted_milli = i128::from(quoted_minor) * 1000;
    let diff = (quoted_milli - i128::from(last_milli)) * 10_000;
    let den = i128::from(last_milli);
    // Rounded half away from zero.
    let q = if diff >= 0 {
        (diff * 2 + den) / (den * 2)
    } else {
        -((-diff * 2 + den) / (den * 2))
    };
    i64::try_from(q).ok()
}

/// Whether a price change is above the customer's alert line (`buying_criteria.price_alert_bps`).
/// Reference only: it never changes compliance, ranking or any decision. False when either
/// side is unknown (no alert line set, or prices in different currencies).
pub fn above_alert(change_bps: Option<i64>, alert_bps: Option<i64>) -> bool {
    matches!((change_bps, alert_bps), (Some(c), Some(a)) if c > a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_string()).collect()
    }

    #[test]
    fn japanese_and_english_headers_are_recognised() {
        let cols = detect_columns(&s(&[
            "伝票日付",
            "仕入先名",
            "品名",
            "型番",
            "数量",
            "単価（円）",
            "金額(税抜)",
            "備考",
            "担当",
        ]));
        assert_eq!(
            cols,
            vec![
                Some(Field::Date),
                Some(Field::Supplier),
                Some(Field::Description),
                Some(Field::Mpn),
                Some(Field::Quantity),
                Some(Field::UnitPrice),
                Some(Field::LineTotal),
                Some(Field::Notes),
                None,
            ]
        );
        let cols = detect_columns(&s(&[
            "Order Date",
            "Vendor",
            "Part No.",
            "Qty",
            "Unit Price",
            "Price",
        ]));
        assert_eq!(cols[2], Some(Field::Mpn));
        // The second "price" column is not taken twice.
        assert_eq!(cols[4], Some(Field::UnitPrice));
        assert_eq!(cols[5], None);
        // Full-width letters in headers.
        assert_eq!(detect_columns(&s(&["ＭＰＮ"])), vec![Some(Field::Mpn)]);
    }

    #[test]
    fn header_row_is_found_below_a_title_block() {
        let rows = vec![
            s(&["購買実績一覧", "", ""]),
            s(&["出力日 2026/10/01", "", ""]),
            s(&["日付", "品名", "数量", "単価"]),
            s(&["2026/1/5", "六角ボルト", "100", "12"]),
        ];
        assert_eq!(find_header(&rows), Some(2));
        assert_eq!(find_header(&[s(&["a", "b"])]), None);
    }

    #[test]
    fn dates_in_common_japanese_formats() {
        let d = |y, m, dd| Day::new(y, m, dd);
        assert_eq!(parse_date("2024/1/5"), d(2024, 1, 5));
        assert_eq!(parse_date("2024-01-05"), d(2024, 1, 5));
        assert_eq!(parse_date("2024.1.5"), d(2024, 1, 5));
        assert_eq!(parse_date("2024年1月5日"), d(2024, 1, 5));
        assert_eq!(parse_date("２０２４／０１／０５"), d(2024, 1, 5));
        assert_eq!(parse_date("2024-01-05T09:30:00"), d(2024, 1, 5));
        assert_eq!(parse_date("令和6年1月5日"), d(2024, 1, 5));
        assert_eq!(parse_date("R6.1.5"), d(2024, 1, 5));
        assert_eq!(parse_date("H31/4/30"), d(2019, 4, 30));
        assert_eq!(parse_date("令和元年5月1日"), d(2019, 5, 1));
        // Excel serial numbers (1900 system): 45296 = 2024-01-05.
        assert_eq!(parse_date("45296"), d(2024, 1, 5));
        assert_eq!(parse_date("45296.5"), d(2024, 1, 5));
        assert_eq!(parse_date("2023/2/29"), None);
        assert_eq!(parse_date("2024/13/1"), None);
        assert_eq!(parse_date("1/5"), None);
        assert_eq!(parse_date("昨日"), None);
        assert_eq!(parse_date(""), None);
    }

    #[test]
    fn epoch_day_conversion_round_trips() {
        for (y, m, dd) in [(1990, 1, 1), (2000, 2, 29), (2024, 12, 31), (2100, 12, 31)] {
            let day = Day::new(y, m, dd).unwrap();
            assert_eq!(Day::from_epoch_days(day.epoch_days()), Some(day));
        }
    }

    fn mapping() -> Vec<Option<Field>> {
        detect_columns(&s(&[
            "日付",
            "仕入先",
            "品名",
            "型番",
            "数量",
            "単価",
            "金額",
            "通貨",
        ]))
    }

    #[test]
    fn rows_are_read_exactly() {
        let r = parse_row(
            &s(&[
                "2026/3/2",
                "山田商事",
                "六角ボルト M8",
                "ＢＴ－Ｍ８",
                "1,000",
                "¥12.5",
                "",
                "",
            ]),
            &mapping(),
            Currency::Jpy,
        )
        .unwrap();
        assert_eq!(r.purchased_on, Day::new(2026, 3, 2).unwrap());
        assert_eq!(r.supplier, "山田商事");
        assert_eq!(r.mpn, "BT-M8");
        assert_eq!(r.quantity_milli, 1_000_000);
        assert_eq!(r.unit_price_milli, 12_500);
        assert!(!r.price_derived);
        assert_eq!(r.currency, Currency::Jpy);

        // Unit price worked out from the total: 1000 / 3 = 333.333 yen.
        let r = parse_row(
            &s(&["2026/3/2", "", "治具", "", "3個", "", "1,000円", ""]),
            &mapping(),
            Currency::Jpy,
        )
        .unwrap();
        assert_eq!(r.quantity_milli, 3000);
        assert_eq!(r.unit_price_milli, 333_333);
        assert!(r.price_derived);

        // USD cents with three more digits; currency from the cell.
        let r = parse_row(
            &s(&[
                "2026-03-02",
                "Acme",
                "Sensor",
                "",
                "2",
                "$1.2345",
                "",
                "usd",
            ]),
            &mapping(),
            Currency::Jpy,
        )
        .unwrap();
        assert_eq!(r.currency, Currency::Usd);
        assert_eq!(r.unit_price_milli, 123_450);
    }

    #[test]
    fn unreadable_rows_are_rejected_with_a_reason() {
        let m = mapping();
        let row = |v: &[&str]| parse_row(&s(v), &m, Currency::Jpy).unwrap_err();
        assert_eq!(
            row(&["", "a", "b", "", "1", "1", "", ""]),
            RowError::DateMissing
        );
        assert_eq!(
            row(&["先月", "a", "b", "", "1", "1", "", ""]),
            RowError::DateInvalid
        );
        assert_eq!(
            row(&["2026/1/1", "a", "", "", "1", "1", "", ""]),
            RowError::ItemMissing
        );
        assert_eq!(
            row(&["2026/1/1", "a", "b", "", "0", "1", "", ""]),
            RowError::QuantityInvalid
        );
        assert_eq!(
            row(&["2026/1/1", "a", "b", "", "-2", "1", "", ""]),
            RowError::QuantityInvalid
        );
        assert_eq!(
            row(&["2026/1/1", "a", "b", "", "1", "", "", ""]),
            RowError::PriceMissing
        );
        assert_eq!(
            row(&["2026/1/1", "a", "b", "", "1", "-5", "", ""]),
            RowError::PriceInvalid
        );
        assert_eq!(
            row(&["2026/1/1", "a", "b", "", "1", "1.23456", "", ""]),
            RowError::PriceInvalid
        );
        assert_eq!(
            row(&["2026/1/1", "a", "b", "", "1", "1", "", "XYZ"]),
            RowError::CurrencyInvalid
        );
        assert!(is_blank(&s(&["", " ", ""])));
    }

    #[test]
    fn items_match_on_part_number_then_description() {
        assert_eq!(item_key("ＢＴ－Ｍ８ x", "whatever"), Some("m:BTM8X".into()));
        assert_eq!(item_key("bt-m8x", ""), item_key("BT M8 X", "other"));
        assert_eq!(
            item_key("", "  六角ボルト　 Ｍ８ "),
            Some("d:六角ボルト m8".into())
        );
        assert_eq!(item_key(" - ", "  "), None);
    }

    #[test]
    fn summary_and_price_change() {
        let p = |y, m, d, sup: &str, price, c| PricePoint {
            purchased_on: Day::new(y, m, d).unwrap(),
            supplier: sup.into(),
            unit_price_milli: price,
            currency: c,
        };
        let pts = vec![
            p(2025, 1, 10, "A", 12_000, Currency::Jpy),
            p(2025, 6, 1, "B", 10_000, Currency::Jpy),
            p(2026, 2, 1, "A", 13_000, Currency::Jpy),
            p(2024, 1, 1, "C", 1, Currency::Usd),
        ];
        let s = summarise(&pts).unwrap();
        assert_eq!(s.purchases, 4);
        assert_eq!(s.last.supplier, "A");
        assert_eq!(s.last.unit_price_milli, 13_000);
        assert_eq!((s.min_price_milli, s.max_price_milli), (10_000, 13_000));
        assert_eq!(s.cheapest.supplier, "B");
        assert_eq!(s.suppliers, 3);
        assert_eq!(summarise(&[]), None);

        // ¥14 quoted against ¥13 last time: +7.69 %.
        assert_eq!(
            change_bps(13_000, Currency::Jpy, 14, Currency::Jpy),
            Some(769)
        );
        assert_eq!(
            change_bps(13_000, Currency::Jpy, 12, Currency::Jpy),
            Some(-769)
        );
        assert_eq!(change_bps(13_000, Currency::Jpy, 14, Currency::Usd), None);
    }

    #[test]
    fn alert_line_is_strictly_above_and_needs_both_sides() {
        assert!(above_alert(Some(1001), Some(1000)));
        assert!(!above_alert(Some(1000), Some(1000)));
        assert!(!above_alert(Some(-5000), Some(1000)));
        assert!(above_alert(Some(1), Some(0)));
        assert!(!above_alert(None, Some(1000)));
        assert!(!above_alert(Some(9999), None));
        assert!(!above_alert(None, None));
    }
}
