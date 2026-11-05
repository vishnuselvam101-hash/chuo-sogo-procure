//! ERP-neutral import records (docs/agents/INTEGRATIONS.md). A factory's purchase orders,
//! goods receipts and supplier invoices arrive as a table (CSV / TSV / xlsx export of its ERP
//! or accounting system); a named column-mapping profile says which column holds which field
//! and how dates and numbers are written. Rows become records with integer money (minor
//! units) and calendar dates, or row errors (row number, field, code). Deterministic: a cell
//! that cannot be read exactly is an error, never a guess.
//!
//! Built-in presets are generic (`generic_ja_po`, …): they describe common Japanese and
//! English header names, not any particular vendor's product.

use crate::history::{Day, half_width, header_key, is_blank, parse_date};
use crate::matching::TaxRounding;
use crate::money::Currency;

/// Data rows one import reads.
pub const MAX_DATA_ROWS: usize = 5_000;
/// Records (orders, receipts, invoices) one import may apply.
pub const MAX_RECORDS: usize = 500;
/// Lines on one record (`procure_server::api::requests::MAX_ITEMS`).
pub const MAX_LINES_PER_RECORD: usize = 200;
/// Explicit columns one profile may name.
pub const MAX_PROFILE_COLUMNS: usize = 60;
/// Quantities are whole units, as on orders.
pub const MAX_QUANTITY: u32 = 1_000_000_000;
/// Largest amount in minor units (as `history::MAX_PRICE_MILLI` / 1000).
pub const MAX_AMOUNT_MINOR: i64 = 1_000_000_000_000;

/// What one file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImportKind {
    Po,
    Receipt,
    Invoice,
}

impl ImportKind {
    pub const ALL: [ImportKind; 3] = [ImportKind::Po, ImportKind::Receipt, ImportKind::Invoice];

    pub const fn code(self) -> &'static str {
        match self {
            ImportKind::Po => "po",
            ImportKind::Receipt => "receipt",
            ImportKind::Invoice => "invoice",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.code() == code)
    }
}

/// A field of an ERP-neutral record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    PoNumber,
    RequestNumber,
    OrderDate,
    ReceivedDate,
    InvoiceNumber,
    InvoiceDate,
    DueDate,
    LineNo,
    Mpn,
    Description,
    Quantity,
    UnitPrice,
    LineAmount,
    Currency,
    TaxRate,
    TaxAmount,
    Shipping,
    Note,
}

impl Field {
    pub const ALL: [Field; 18] = [
        Field::PoNumber,
        Field::RequestNumber,
        Field::OrderDate,
        Field::ReceivedDate,
        Field::InvoiceNumber,
        Field::InvoiceDate,
        Field::DueDate,
        Field::LineNo,
        Field::Mpn,
        Field::Description,
        Field::Quantity,
        Field::UnitPrice,
        Field::LineAmount,
        Field::Currency,
        Field::TaxRate,
        Field::TaxAmount,
        Field::Shipping,
        Field::Note,
    ];

    pub const fn code(self) -> &'static str {
        match self {
            Field::PoNumber => "po_number",
            Field::RequestNumber => "request_number",
            Field::OrderDate => "order_date",
            Field::ReceivedDate => "received_date",
            Field::InvoiceNumber => "invoice_number",
            Field::InvoiceDate => "invoice_date",
            Field::DueDate => "due_date",
            Field::LineNo => "line_no",
            Field::Mpn => "mpn",
            Field::Description => "description",
            Field::Quantity => "quantity",
            Field::UnitPrice => "unit_price",
            Field::LineAmount => "line_amount",
            Field::Currency => "currency",
            Field::TaxRate => "tax_rate",
            Field::TaxAmount => "tax_amount",
            Field::Shipping => "shipping",
            Field::Note => "note",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.code() == code)
    }

    /// Header names (normalised by `header_key`: lower case, letters and digits only, a
    /// bracketed suffix dropped) recognised for this field, Japanese and English.
    const fn synonyms(self) -> &'static [&'static str] {
        match self {
            Field::PoNumber => &[
                "発注番号",
                "注文番号",
                "発注no",
                "注文no",
                "発注書番号",
                "注文書番号",
                "po番号",
                "ponumber",
                "pono",
                "po",
                "purchaseorder",
                "purchaseordernumber",
                "orderno",
                "ordernumber",
            ],
            Field::RequestNumber => &[
                "依頼番号",
                "購買依頼番号",
                "購入依頼番号",
                "依頼no",
                "requestnumber",
                "requestno",
                "prnumber",
                "prno",
            ],
            Field::OrderDate => &["発注日", "注文日", "発注日付", "orderdate", "podate"],
            Field::ReceivedDate => &[
                "納品日",
                "入荷日",
                "受入日",
                "検収日",
                "着荷日",
                "receiveddate",
                "receiptdate",
                "receivedon",
                "deliverydate",
            ],
            Field::InvoiceNumber => &[
                "請求番号",
                "請求書番号",
                "請求書no",
                "請求no",
                "invoicenumber",
                "invoiceno",
            ],
            Field::InvoiceDate => &["請求日", "請求日付", "請求書日付", "invoicedate"],
            Field::DueDate => &[
                "支払期日",
                "支払予定日",
                "支払期限",
                "duedate",
                "paymentdue",
            ],
            Field::LineNo => &[
                "行番号",
                "明細番号",
                "明細行",
                "明細no",
                "行no",
                "lineno",
                "linenumber",
                "line",
            ],
            Field::Mpn => &[
                "品番",
                "型番",
                "品目コード",
                "品目番号",
                "部品番号",
                "型式",
                "mpn",
                "partnumber",
                "partno",
                "itemcode",
                "sku",
            ],
            Field::Description => &[
                "品名",
                "品目",
                "品目名",
                "名称",
                "商品名",
                "description",
                "itemname",
                "item",
            ],
            Field::Quantity => &[
                "数量",
                "発注数",
                "発注数量",
                "入荷数",
                "入荷数量",
                "納品数",
                "納品数量",
                "受入数",
                "受入数量",
                "請求数量",
                "quantity",
                "qty",
                "receivedqty",
            ],
            Field::UnitPrice => &[
                "単価",
                "発注単価",
                "仕入単価",
                "請求単価",
                "unitprice",
                "price",
            ],
            Field::LineAmount => &[
                "金額",
                "明細金額",
                "税抜金額",
                "amount",
                "lineamount",
                "linetotal",
                "extendedprice",
            ],
            Field::Currency => &["通貨", "currency"],
            Field::TaxRate => &["税率", "消費税率", "taxrate", "vatrate"],
            Field::TaxAmount => &["消費税", "消費税額", "税額", "taxamount", "tax", "vat"],
            Field::Shipping => &["送料", "運賃", "配送料", "shipping", "freight"],
            Field::Note => &["備考", "メモ", "摘要", "note", "notes", "remarks", "memo"],
        }
    }
}

/// How the file writes dates. `Auto` reads every form `history::parse_date` knows
/// (`2026/1/5`, `2026年1月5日`, `令和8年1月5日`, Excel serials); the others are for exports
/// that write the day first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateFormat {
    Auto,
    Ymd,
    Dmy,
    Mdy,
}

impl DateFormat {
    pub const fn code(self) -> &'static str {
        match self {
            DateFormat::Auto => "auto",
            DateFormat::Ymd => "ymd",
            DateFormat::Dmy => "dmy",
            DateFormat::Mdy => "mdy",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        [
            DateFormat::Auto,
            DateFormat::Ymd,
            DateFormat::Dmy,
            DateFormat::Mdy,
        ]
        .into_iter()
        .find(|f| f.code() == code)
    }
}

/// Character encoding of a CSV / TSV file (`Auto`: UTF-8 if valid, else Shift_JIS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    Auto,
    Utf8,
    ShiftJis,
}

impl TextEncoding {
    pub const fn code(self) -> &'static str {
        match self {
            TextEncoding::Auto => "auto",
            TextEncoding::Utf8 => "utf-8",
            TextEncoding::ShiftJis => "shift_jis",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        [
            TextEncoding::Auto,
            TextEncoding::Utf8,
            TextEncoding::ShiftJis,
        ]
        .into_iter()
        .find(|e| e.code() == code)
    }
}

/// A named column-mapping profile, saved per organisation (or a built-in preset).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// `[a-z0-9_]{1,60}`.
    pub name: String,
    pub kind: ImportKind,
    /// Source column header → field. Columns not named here are recognised by their header
    /// (Japanese / English synonyms) unless that field is already taken.
    pub columns: Vec<(String, Field)>,
    pub date_format: DateFormat,
    /// `.` or `,`.
    pub decimal_separator: char,
    /// `,`, `.`, ` `, `'` or none; never the decimal separator.
    pub thousands_separator: Option<char>,
    pub encoding: TextEncoding,
    /// Rate used for invoices whose rows carry no tax rate (basis points; 1000 = 10%).
    /// `None`: no default; an invoice without a rate is refused.
    pub default_tax_rate_bps: Option<u32>,
    /// How the customer's suppliers round invoice tax (recorded with each imported invoice
    /// and used by the three-way match).
    pub tax_rounding: TaxRounding,
}

/// Why a profile cannot be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileError {
    Name,
    Columns,
    ColumnName,
    DuplicateField,
    Separators,
    TaxRate,
}

impl ProfileError {
    pub const fn code(self) -> &'static str {
        match self {
            ProfileError::Name => "profile_name",
            ProfileError::Columns => "profile_columns",
            ProfileError::ColumnName => "profile_column_name",
            ProfileError::DuplicateField => "profile_duplicate_field",
            ProfileError::Separators => "profile_separators",
            ProfileError::TaxRate => "profile_tax_rate",
        }
    }
}

/// A valid profile name: `[a-z0-9_]{1,60}`.
pub fn valid_profile_name(name: &str) -> bool {
    (1..=60).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

impl Profile {
    /// Defaults for `kind`: headers detected, dates auto, `1,234.5`, encoding auto, 10% tax
    /// rounded down (the common Japanese qualified-invoice rule).
    pub fn detect(name: &str, kind: ImportKind) -> Self {
        Profile {
            name: name.into(),
            kind,
            columns: Vec::new(),
            date_format: DateFormat::Auto,
            decimal_separator: '.',
            thousands_separator: Some(','),
            encoding: TextEncoding::Auto,
            default_tax_rate_bps: Some(1000),
            tax_rounding: TaxRounding::Floor,
        }
    }

    pub fn validate(&self) -> Result<(), ProfileError> {
        if !valid_profile_name(&self.name) {
            return Err(ProfileError::Name);
        }
        if self.columns.len() > MAX_PROFILE_COLUMNS {
            return Err(ProfileError::Columns);
        }
        for (i, (source, field)) in self.columns.iter().enumerate() {
            if header_key(source).is_empty()
                || source.chars().count() > 100
                || source.chars().any(char::is_control)
            {
                return Err(ProfileError::ColumnName);
            }
            if self.columns[..i].iter().any(|(_, f)| f == field) {
                return Err(ProfileError::DuplicateField);
            }
        }
        let thousands_ok = match self.thousands_separator {
            None => true,
            Some(t) => [',', '.', ' ', '\''].contains(&t) && t != self.decimal_separator,
        };
        if !['.', ','].contains(&self.decimal_separator) || !thousands_ok {
            return Err(ProfileError::Separators);
        }
        if self.default_tax_rate_bps.is_some_and(|r| r > 10_000) {
            return Err(ProfileError::TaxRate);
        }
        Ok(())
    }
}

/// Built-in generic profiles. They name common header words; any vendor's export with other
/// headers needs a saved profile of its own.
pub fn presets() -> Vec<Profile> {
    use Field::*;
    let ja = |name: &str, kind, cols: &[(&str, Field)]| Profile {
        columns: cols.iter().map(|(s, f)| ((*s).to_string(), *f)).collect(),
        ..Profile::detect(name, kind)
    };
    vec![
        ja(
            "generic_ja_po",
            ImportKind::Po,
            &[
                ("発注番号", PoNumber),
                ("依頼番号", RequestNumber),
                ("発注日", OrderDate),
                ("行番号", LineNo),
                ("品番", Mpn),
                ("品名", Description),
                ("数量", Quantity),
                ("単価", UnitPrice),
                ("金額", LineAmount),
            ],
        ),
        ja(
            "generic_ja_receipt",
            ImportKind::Receipt,
            &[
                ("発注番号", PoNumber),
                ("納品日", ReceivedDate),
                ("行番号", LineNo),
                ("品番", Mpn),
                ("品名", Description),
                ("数量", Quantity),
                ("備考", Note),
            ],
        ),
        ja(
            "generic_ja_invoice",
            ImportKind::Invoice,
            &[
                ("請求番号", InvoiceNumber),
                ("発注番号", PoNumber),
                ("請求日", InvoiceDate),
                ("支払期日", DueDate),
                ("行番号", LineNo),
                ("品番", Mpn),
                ("品名", Description),
                ("数量", Quantity),
                ("単価", UnitPrice),
                ("金額", LineAmount),
                ("税率", TaxRate),
                ("消費税", TaxAmount),
                ("送料", Shipping),
            ],
        ),
        ja(
            "generic_en_po",
            ImportKind::Po,
            &[
                ("PO Number", PoNumber),
                ("Request Number", RequestNumber),
                ("Order Date", OrderDate),
                ("Line", LineNo),
                ("Part Number", Mpn),
                ("Description", Description),
                ("Quantity", Quantity),
                ("Unit Price", UnitPrice),
                ("Amount", LineAmount),
            ],
        ),
        ja(
            "generic_en_receipt",
            ImportKind::Receipt,
            &[
                ("PO Number", PoNumber),
                ("Received Date", ReceivedDate),
                ("Line", LineNo),
                ("Part Number", Mpn),
                ("Description", Description),
                ("Quantity", Quantity),
                ("Note", Note),
            ],
        ),
        ja(
            "generic_en_invoice",
            ImportKind::Invoice,
            &[
                ("Invoice Number", InvoiceNumber),
                ("PO Number", PoNumber),
                ("Invoice Date", InvoiceDate),
                ("Due Date", DueDate),
                ("Line", LineNo),
                ("Part Number", Mpn),
                ("Description", Description),
                ("Quantity", Quantity),
                ("Unit Price", UnitPrice),
                ("Amount", LineAmount),
                ("Tax Rate", TaxRate),
                ("Tax", TaxAmount),
                ("Shipping", Shipping),
            ],
        ),
    ]
}

/// The built-in preset called `name`.
pub fn preset(name: &str) -> Option<Profile> {
    presets().into_iter().find(|p| p.name == name)
}

/// Which field each header cell holds (`None` = ignored). The profile's explicit columns come
/// first; other cells are recognised by synonym. A field is taken by its first column.
pub fn map_columns(header: &[String], profile: &Profile) -> Vec<Option<Field>> {
    let explicit: Vec<(String, Field)> = profile
        .columns
        .iter()
        .map(|(s, f)| (header_key(s), *f))
        .collect();
    let mut taken: Vec<Field> = Vec::new();
    let mut out: Vec<Option<Field>> = header
        .iter()
        .map(|h| {
            let key = header_key(h);
            let f = explicit.iter().find(|(s, _)| *s == key).map(|(_, f)| *f)?;
            if taken.contains(&f) {
                return None;
            }
            taken.push(f);
            Some(f)
        })
        .collect();
    for (i, h) in header.iter().enumerate() {
        if out[i].is_some() {
            continue;
        }
        let key = header_key(h);
        if key.is_empty() || explicit.iter().any(|(s, _)| *s == key) {
            continue;
        }
        if let Some(f) = Field::ALL
            .into_iter()
            .find(|f| !taken.contains(f) && f.synonyms().contains(&key.as_str()))
        {
            taken.push(f);
            out[i] = Some(f);
        }
    }
    out
}

/// Fields a file of `kind` must have a column for.
pub fn required_fields(kind: ImportKind) -> &'static [Field] {
    match kind {
        ImportKind::Po => &[Field::PoNumber, Field::RequestNumber],
        ImportKind::Receipt => &[Field::PoNumber, Field::ReceivedDate, Field::Quantity],
        ImportKind::Invoice => &[
            Field::InvoiceNumber,
            Field::PoNumber,
            Field::InvoiceDate,
            Field::Quantity,
            Field::TaxAmount,
        ],
    }
}

/// Required fields `mapping` lacks (an item column and, for invoices, a price column count too).
pub fn missing_fields(kind: ImportKind, mapping: &[Option<Field>]) -> Vec<Field> {
    let has = |f: Field| mapping.contains(&Some(f));
    let mut missing: Vec<Field> = required_fields(kind)
        .iter()
        .copied()
        .filter(|f| !has(*f))
        .collect();
    let names_item = has(Field::LineNo) || has(Field::Mpn) || has(Field::Description);
    if kind != ImportKind::Po && !names_item {
        missing.push(Field::Mpn);
    }
    if kind == ImportKind::Invoice && !has(Field::UnitPrice) && !has(Field::LineAmount) {
        missing.push(Field::UnitPrice);
    }
    missing
}

/// The header row: the first of the leading rows whose mapping has every required field
/// (exports often start with a title block).
pub fn find_header(rows: &[Vec<String>], profile: &Profile) -> Option<usize> {
    rows.iter()
        .take(15)
        .position(|row| missing_fields(profile.kind, &map_columns(row, profile)).is_empty())
}

/// Why a cell or row was not read. Codes are shown to the person importing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    Required,
    DateInvalid,
    NumberInvalid,
    QuantityInvalid,
    AmountInvalid,
    AmountMismatch,
    CurrencyInvalid,
    TaxRateInvalid,
    TextTooLong,
    Inconsistent,
    DuplicateLine,
    TooManyLines,
    TooManyRecords,
    TooManyRows,
    ItemMissing,
}

impl ErrorCode {
    pub const fn code(self) -> &'static str {
        match self {
            ErrorCode::Required => "required",
            ErrorCode::DateInvalid => "date_invalid",
            ErrorCode::NumberInvalid => "number_invalid",
            ErrorCode::QuantityInvalid => "quantity_invalid",
            ErrorCode::AmountInvalid => "amount_invalid",
            ErrorCode::AmountMismatch => "amount_mismatch",
            ErrorCode::CurrencyInvalid => "currency_invalid",
            ErrorCode::TaxRateInvalid => "tax_rate_invalid",
            ErrorCode::TextTooLong => "text_too_long",
            ErrorCode::Inconsistent => "inconsistent",
            ErrorCode::DuplicateLine => "duplicate_line",
            ErrorCode::TooManyLines => "too_many_lines",
            ErrorCode::TooManyRecords => "too_many_records",
            ErrorCode::TooManyRows => "too_many_rows",
            ErrorCode::ItemMissing => "item_missing",
        }
    }
}

/// One problem: file row (1-based, as a spreadsheet numbers it), field, code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowError {
    pub row: usize,
    pub field: Option<Field>,
    pub code: ErrorCode,
}

/// One item line of a record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordLine {
    /// File row (1-based).
    pub row: usize,
    /// The request's line number, when the file has one.
    pub line_no: Option<u32>,
    pub mpn: String,
    pub description: String,
    pub quantity: Option<u32>,
    /// Minor units of the record's currency.
    pub unit_price_minor: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurchaseOrderRecord {
    pub po_number: String,
    /// The platform request the factory's PO is for (`PR-…`).
    pub request_number: String,
    pub order_date: Option<Day>,
    pub currency: Currency,
    pub lines: Vec<RecordLine>,
    pub rows: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptRecord {
    pub po_number: String,
    pub received_on: Day,
    pub note: String,
    pub lines: Vec<RecordLine>,
    pub rows: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvoiceRecord {
    pub invoice_number: String,
    pub po_number: String,
    pub invoice_date: Day,
    pub due_date: Option<Day>,
    pub currency: Currency,
    pub tax_rate_bps: u32,
    pub tax_rounding: TaxRounding,
    /// Tax as printed on the invoice.
    pub tax_minor: i64,
    pub shipping_minor: i64,
    pub lines: Vec<RecordLine>,
    pub rows: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
    Po(PurchaseOrderRecord),
    Receipt(ReceiptRecord),
    Invoice(InvoiceRecord),
}

impl Record {
    pub fn po_number(&self) -> &str {
        match self {
            Record::Po(r) => &r.po_number,
            Record::Receipt(r) => &r.po_number,
            Record::Invoice(r) => &r.po_number,
        }
    }

    pub fn lines(&self) -> &[RecordLine] {
        match self {
            Record::Po(r) => &r.lines,
            Record::Receipt(r) => &r.lines,
            Record::Invoice(r) => &r.lines,
        }
    }

    pub fn rows(&self) -> &[usize] {
        match self {
            Record::Po(r) => &r.rows,
            Record::Receipt(r) => &r.rows,
            Record::Invoice(r) => &r.rows,
        }
    }
}

/// What a table yields: records whose every row was read, and the errors of the others (a
/// record with one bad row is not imported at all, so nothing is half-applied).
#[derive(Debug, Default)]
pub struct Parsed {
    pub records: Vec<Record>,
    pub errors: Vec<RowError>,
    /// Non-blank data rows read.
    pub data_rows: usize,
}

fn text(raw: &str, max: usize) -> Result<String, ErrorCode> {
    let t: String = raw.trim().chars().filter(|c| !c.is_control()).collect();
    if t.chars().count() > max {
        return Err(ErrorCode::TextTooLong);
    }
    Ok(t)
}

/// Digits of a non-negative number as an integer scaled by 10^`scale`, read with the
/// profile's separators; `None` if not exactly that. Currency signs before and a unit word
/// after (`円`, `個`, `pcs`) are allowed.
pub fn parse_number(raw: &str, scale: u32, profile: &Profile) -> Option<i64> {
    let s = half_width(raw.trim());
    let s = s
        .trim_start_matches(['¥', '￥', '$', '€', '₹', '\\'])
        .trim();
    let is_num = |c: char| {
        c.is_ascii_digit()
            || c == profile.decimal_separator
            || Some(c) == profile.thousands_separator
    };
    let end = s
        .char_indices()
        .find(|(_, c)| !is_num(*c))
        .map_or(s.len(), |(i, _)| i);
    let (num, rest) = s.split_at(end);
    let rest = rest.trim();
    if num.is_empty()
        || rest
            .chars()
            .any(|c| c.is_ascii_digit() || c == '-' || c == '.')
    {
        return None;
    }
    let num = num.trim_end_matches(' ');
    let mut whole = String::new();
    let mut frac: Option<String> = None;
    for c in num.chars() {
        if c == profile.decimal_separator {
            if frac.is_some() {
                return None;
            }
            frac = Some(String::new());
        } else if Some(c) == profile.thousands_separator {
            if frac.is_some() {
                return None;
            }
        } else {
            match &mut frac {
                Some(f) => f.push(c),
                None => whole.push(c),
            }
        }
    }
    let frac = frac.unwrap_or_default();
    if whole.is_empty() || whole.len() > 15 || frac.len() > scale as usize {
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

/// A date in the profile's format.
pub fn parse_day(raw: &str, format: DateFormat) -> Option<Day> {
    match format {
        DateFormat::Auto | DateFormat::Ymd => parse_date(raw),
        DateFormat::Dmy | DateFormat::Mdy => {
            let s = half_width(raw.trim());
            let date = s.split(['T', ' ']).next().unwrap_or("");
            let parts: Vec<i64> = date
                .split(|c: char| !c.is_ascii_digit())
                .filter(|p| !p.is_empty())
                .filter_map(|p| p.parse().ok())
                .collect();
            let &[a, b, y] = &parts[..] else {
                return None;
            };
            let (d, m) = if format == DateFormat::Dmy {
                (a, b)
            } else {
                (b, a)
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
    }
}

fn currency_of(raw: &str) -> Option<Currency> {
    let s = half_width(raw.trim()).to_uppercase();
    match s.as_str() {
        "円" | "¥" | "￥" => Some(Currency::Jpy),
        "$" => Some(Currency::Usd),
        "€" => Some(Currency::Eur),
        "₹" => Some(Currency::Inr),
        _ => Currency::from_code(&s),
    }
}

/// `10%`, `10`, `8` → percent; `0.1`, `0.08` (below one, no `%`) → a fraction. Basis points.
pub fn parse_tax_rate(raw: &str, profile: &Profile) -> Option<u32> {
    let s = half_width(raw.trim());
    let (num, percent) = match s.strip_suffix('%') {
        Some(n) => (n.trim(), true),
        None => (s.as_str(), false),
    };
    let x = parse_number(num, 4, profile)?;
    let bps = if !percent && x < 10_000 {
        x
    } else if x % 100 == 0 {
        x / 100
    } else {
        return None;
    };
    u32::try_from(bps).ok().filter(|b| *b <= 10_000)
}

/// Header values one row gives its record, and its item line.
#[derive(Debug, Default)]
struct RowValues {
    row: usize,
    key: String,
    po_number: String,
    request_number: String,
    invoice_number: String,
    date: Option<Day>,
    due_date: Option<Day>,
    currency: Option<Currency>,
    tax_rate_bps: Option<u32>,
    tax_minor: Option<i64>,
    shipping_minor: Option<i64>,
    note: String,
    line: Option<RecordLine>,
}

fn read_row(
    cells: &[String],
    row: usize,
    mapping: &[Option<Field>],
    profile: &Profile,
    default_currency: Currency,
) -> Result<RowValues, Vec<RowError>> {
    let get = |f: Field| -> &str {
        mapping
            .iter()
            .position(|m| *m == Some(f))
            .and_then(|i| cells.get(i))
            .map_or("", |s| s.trim())
    };
    let mut errors = Vec::new();
    let mut err = |field: Field, code: ErrorCode| {
        errors.push(RowError {
            row,
            field: Some(field),
            code,
        })
    };
    let kind = profile.kind;
    let mut v = RowValues {
        row,
        ..RowValues::default()
    };
    let txt =
        |f: Field, max: usize, required: bool, err: &mut dyn FnMut(Field, ErrorCode)| match text(
            &half_width(get(f)),
            max,
        ) {
            Ok(t) if t.is_empty() && required => {
                err(f, ErrorCode::Required);
                String::new()
            }
            Ok(t) => t,
            Err(c) => {
                err(f, c);
                String::new()
            }
        };
    v.po_number = txt(Field::PoNumber, 100, true, &mut err);
    match kind {
        ImportKind::Po => v.request_number = txt(Field::RequestNumber, 40, true, &mut err),
        ImportKind::Invoice => v.invoice_number = txt(Field::InvoiceNumber, 100, true, &mut err),
        ImportKind::Receipt => {}
    }
    let date_field = match kind {
        ImportKind::Po => Field::OrderDate,
        ImportKind::Receipt => Field::ReceivedDate,
        ImportKind::Invoice => Field::InvoiceDate,
    };
    let day = |f: Field, required: bool, err: &mut dyn FnMut(Field, ErrorCode)| match get(f) {
        "" if required => {
            err(f, ErrorCode::Required);
            None
        }
        "" => None,
        d => {
            let parsed = parse_day(d, profile.date_format);
            if parsed.is_none() {
                err(f, ErrorCode::DateInvalid);
            }
            parsed
        }
    };
    v.date = day(date_field, kind != ImportKind::Po, &mut err);
    if kind == ImportKind::Invoice {
        v.due_date = day(Field::DueDate, false, &mut err);
    }
    let currency = match get(Field::Currency) {
        "" => default_currency,
        c => match currency_of(c) {
            Some(c) => {
                v.currency = Some(c);
                c
            }
            None => {
                err(Field::Currency, ErrorCode::CurrencyInvalid);
                default_currency
            }
        },
    };
    let digits = currency.minor_digits();
    let amount = |f: Field, err: &mut dyn FnMut(Field, ErrorCode)| match get(f) {
        "" => None,
        a => {
            let parsed = parse_number(a, digits, profile).filter(|a| *a <= MAX_AMOUNT_MINOR);
            if parsed.is_none() {
                err(f, ErrorCode::AmountInvalid);
            }
            parsed
        }
    };
    if kind == ImportKind::Invoice {
        v.tax_minor = amount(Field::TaxAmount, &mut err);
        v.shipping_minor = amount(Field::Shipping, &mut err);
        v.tax_rate_bps = match get(Field::TaxRate) {
            "" => None,
            r => {
                let parsed = parse_tax_rate(r, profile);
                if parsed.is_none() {
                    err(Field::TaxRate, ErrorCode::TaxRateInvalid);
                }
                parsed
            }
        };
    }
    v.note = txt(Field::Note, 2000, false, &mut err);
    // The item line.
    let line_no = match get(Field::LineNo) {
        "" => None,
        n => {
            let parsed = parse_number(n, 0, profile)
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| (1..=10_000).contains(n));
            if parsed.is_none() {
                err(Field::LineNo, ErrorCode::NumberInvalid);
            }
            parsed
        }
    };
    let mpn = txt(Field::Mpn, 100, false, &mut err);
    let description = txt(Field::Description, 500, false, &mut err);
    let quantity = match get(Field::Quantity) {
        "" if kind != ImportKind::Po => {
            err(Field::Quantity, ErrorCode::Required);
            None
        }
        "" => None,
        q => {
            let parsed = parse_number(q, 0, profile)
                .and_then(|q| u32::try_from(q).ok())
                .filter(|q| (1..=MAX_QUANTITY).contains(q));
            if parsed.is_none() {
                err(Field::Quantity, ErrorCode::QuantityInvalid);
            }
            parsed
        }
    };
    let unit = amount(Field::UnitPrice, &mut err);
    let total = amount(Field::LineAmount, &mut err);
    let unit_price_minor = match (unit, total, quantity) {
        (Some(u), Some(t), Some(q)) => {
            if u.checked_mul(i64::from(q)) != Some(t) {
                err(Field::LineAmount, ErrorCode::AmountMismatch);
            }
            Some(u)
        }
        (Some(u), _, _) => Some(u),
        // Unit price from the line total only when it divides exactly: never rounded.
        (None, Some(t), Some(q)) if t % i64::from(q) == 0 => Some(t / i64::from(q)),
        (None, Some(_), Some(_)) => {
            err(Field::LineAmount, ErrorCode::AmountInvalid);
            None
        }
        _ => None,
    };
    if kind == ImportKind::Invoice && unit.is_none() && total.is_none() && quantity.is_some() {
        err(Field::UnitPrice, ErrorCode::Required);
    }
    let names_item = line_no.is_some() || !mpn.is_empty() || !description.is_empty();
    let has_line = names_item || quantity.is_some() || unit_price_minor.is_some();
    if has_line {
        if !names_item {
            errors.push(RowError {
                row,
                field: None,
                code: ErrorCode::ItemMissing,
            });
        }
        v.line = Some(RecordLine {
            row,
            line_no,
            mpn,
            description,
            quantity,
            unit_price_minor,
        });
    } else if kind != ImportKind::Po {
        errors.push(RowError {
            row,
            field: None,
            code: ErrorCode::ItemMissing,
        });
    }
    v.key = match kind {
        ImportKind::Po => v.po_number.clone(),
        ImportKind::Receipt => format!(
            "{}\u{1f}{}",
            v.po_number,
            v.date.map(Day::iso).unwrap_or_default()
        ),
        ImportKind::Invoice => v.invoice_number.clone(),
    };
    if errors.is_empty() {
        Ok(v)
    } else {
        Err(errors)
    }
}

/// Keeps the first non-empty value of a header field; a different one later is an error.
fn same<T: PartialEq + Clone>(
    slot: &mut Option<T>,
    value: Option<T>,
    row: usize,
    field: Field,
    errors: &mut Vec<RowError>,
) {
    match (slot.as_ref(), value) {
        (_, None) => {}
        (None, Some(v)) => *slot = Some(v),
        (Some(a), Some(b)) if *a == b => {}
        (Some(_), Some(_)) => errors.push(RowError {
            row,
            field: Some(field),
            code: ErrorCode::Inconsistent,
        }),
    }
}

fn some_text(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

/// Reads the data rows after `header` with `mapping`. Rows of one record (same PO number;
/// same PO and receipt date; same invoice number) are grouped; header values must agree on
/// every row of a record. `default_currency` applies when the file has no currency column.
pub fn parse(
    rows: &[Vec<String>],
    header: usize,
    mapping: &[Option<Field>],
    profile: &Profile,
    default_currency: Currency,
) -> Parsed {
    let mut parsed = Parsed::default();
    let data: Vec<(usize, &Vec<String>)> = rows
        .iter()
        .enumerate()
        .skip(header + 1)
        .filter(|(_, r)| !is_blank(r))
        .map(|(i, r)| (i + 1, r))
        .collect();
    parsed.data_rows = data.len();
    if data.len() > MAX_DATA_ROWS {
        parsed.errors.push(RowError {
            row: 0,
            field: None,
            code: ErrorCode::TooManyRows,
        });
        return parsed;
    }
    // (key, row values, rows with errors) in first-seen order.
    let mut groups: Vec<(String, Vec<RowValues>, bool)> = Vec::new();
    for (row, cells) in data {
        let values = read_row(cells, row, mapping, profile, default_currency);
        let key = match &values {
            Ok(v) => v.key.clone(),
            Err(_) => {
                // The group key still counts, so the rest of that record is held back too.
                let po = mapping
                    .iter()
                    .position(|m| {
                        *m == Some(match profile.kind {
                            ImportKind::Invoice => Field::InvoiceNumber,
                            _ => Field::PoNumber,
                        })
                    })
                    .and_then(|i| cells.get(i))
                    .map(|s| half_width(s.trim()))
                    .unwrap_or_default();
                match profile.kind {
                    // A receipt row with a bad date cannot be put with its record: hold back
                    // every receipt of that PO in the file.
                    ImportKind::Receipt => format!("{po}\u{1f}*"),
                    _ => po,
                }
            }
        };
        let gi = match groups.iter().position(|(k, ..)| *k == key) {
            Some(i) => i,
            None => {
                groups.push((key, Vec::new(), false));
                groups.len() - 1
            }
        };
        match values {
            Ok(v) => groups[gi].1.push(v),
            Err(e) => {
                parsed.errors.extend(e);
                groups[gi].2 = true;
            }
        }
    }
    // Receipt rows whose date could not be read hold back every receipt of their PO.
    let held_pos: Vec<String> = groups
        .iter()
        .filter_map(|(k, ..)| k.strip_suffix("\u{1f}*").map(str::to_string))
        .collect();
    for (key, values, bad) in groups {
        if bad || values.is_empty() {
            continue;
        }
        if profile.kind == ImportKind::Receipt
            && held_pos
                .iter()
                .any(|p| key.split('\u{1f}').next() == Some(p))
        {
            continue;
        }
        let mut errors = Vec::new();
        let first = &values[0];
        let mut request = None;
        let mut po = None;
        let mut date = None;
        let mut due = None;
        let mut currency = None;
        let mut rate = None;
        let mut tax = None;
        let mut shipping = None;
        let mut note = None;
        let mut lines: Vec<RecordLine> = Vec::new();
        for v in &values {
            let row = v.row;
            same(
                &mut request,
                some_text(&v.request_number),
                row,
                Field::RequestNumber,
                &mut errors,
            );
            same(
                &mut po,
                some_text(&v.po_number),
                row,
                Field::PoNumber,
                &mut errors,
            );
            same(
                &mut date,
                v.date,
                row,
                match profile.kind {
                    ImportKind::Po => Field::OrderDate,
                    ImportKind::Receipt => Field::ReceivedDate,
                    ImportKind::Invoice => Field::InvoiceDate,
                },
                &mut errors,
            );
            same(&mut due, v.due_date, row, Field::DueDate, &mut errors);
            same(&mut currency, v.currency, row, Field::Currency, &mut errors);
            same(&mut rate, v.tax_rate_bps, row, Field::TaxRate, &mut errors);
            same(&mut tax, v.tax_minor, row, Field::TaxAmount, &mut errors);
            same(
                &mut shipping,
                v.shipping_minor,
                row,
                Field::Shipping,
                &mut errors,
            );
            same(&mut note, some_text(&v.note), row, Field::Note, &mut errors);
            if let Some(l) = &v.line {
                if l.line_no.is_some() && lines.iter().any(|o| o.line_no == l.line_no) {
                    errors.push(RowError {
                        row: l.row,
                        field: Some(Field::LineNo),
                        code: ErrorCode::DuplicateLine,
                    });
                }
                lines.push(l.clone());
            }
        }
        let rows: Vec<usize> = values.iter().map(|v| v.row).collect();
        let last_row = rows.last().copied().unwrap_or(0);
        if lines.len() > MAX_LINES_PER_RECORD {
            errors.push(RowError {
                row: last_row,
                field: None,
                code: ErrorCode::TooManyLines,
            });
        }
        if profile.kind == ImportKind::Invoice && tax.is_none() {
            errors.push(RowError {
                row: rows.first().copied().unwrap_or(0),
                field: Some(Field::TaxAmount),
                code: ErrorCode::Required,
            });
        }
        // No rate in the file and none in the profile: never guessed.
        let rate = rate.or(profile.default_tax_rate_bps);
        if profile.kind == ImportKind::Invoice && rate.is_none() {
            errors.push(RowError {
                row: rows.first().copied().unwrap_or(0),
                field: Some(Field::TaxRate),
                code: ErrorCode::Required,
            });
        }
        if !errors.is_empty() {
            parsed.errors.extend(errors);
            continue;
        }
        let currency = currency.unwrap_or(default_currency);
        let record = match profile.kind {
            ImportKind::Po => Record::Po(PurchaseOrderRecord {
                po_number: po.unwrap_or_else(|| first.po_number.clone()),
                request_number: request.unwrap_or_default(),
                order_date: date,
                currency,
                lines,
                rows,
            }),
            ImportKind::Receipt => Record::Receipt(ReceiptRecord {
                po_number: po.unwrap_or_else(|| first.po_number.clone()),
                // Receipt rows without a date fail in `read_row`.
                received_on: match date {
                    Some(d) => d,
                    None => continue,
                },
                note: note.unwrap_or_default(),
                lines,
                rows,
            }),
            ImportKind::Invoice => Record::Invoice(InvoiceRecord {
                invoice_number: first.invoice_number.clone(),
                po_number: po.unwrap_or_default(),
                invoice_date: match date {
                    Some(d) => d,
                    None => continue,
                },
                due_date: due,
                currency,
                tax_rate_bps: match rate {
                    Some(r) => r,
                    None => continue,
                },
                tax_rounding: profile.tax_rounding,
                tax_minor: tax.unwrap_or(0),
                shipping_minor: shipping.unwrap_or(0),
                lines,
                rows,
            }),
        };
        if parsed.records.len() == MAX_RECORDS {
            parsed.errors.push(RowError {
                row: last_row,
                field: None,
                code: ErrorCode::TooManyRecords,
            });
            continue;
        }
        parsed.records.push(record);
    }
    parsed.errors.sort_by_key(|e| e.row);
    parsed
}

/// An order line a record line may refer to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownLine {
    /// The request line number.
    pub line_no: u32,
    /// Part numbers it is known by (ordered, offered, verified).
    pub mpns: Vec<String>,
    pub description: String,
}

/// The order line `line` means: by line number when the file has one, else by part number
/// (letters and digits, upper case), else by description; `None` when not exactly one.
pub fn match_line(known: &[KnownLine], line: &RecordLine) -> Option<usize> {
    if let Some(n) = line.line_no {
        return known.iter().position(|k| k.line_no == n);
    }
    let unique = |hits: Vec<usize>| (hits.len() == 1).then(|| hits[0]);
    let mpn = norm_mpn(&line.mpn);
    if !mpn.is_empty() {
        return unique(
            known
                .iter()
                .enumerate()
                .filter(|(_, k)| k.mpns.iter().any(|m| norm_mpn(m) == mpn))
                .map(|(i, _)| i)
                .collect(),
        );
    }
    let d = norm_text(&line.description);
    if d.is_empty() {
        return None;
    }
    unique(
        known
            .iter()
            .enumerate()
            .filter(|(_, k)| norm_text(&k.description) == d)
            .map(|(i, _)| i)
            .collect(),
    )
}

fn norm_mpn(s: &str) -> String {
    half_width(s)
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect()
}

fn norm_text(s: &str) -> String {
    half_width(s)
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn rows(t: &[&[&str]]) -> Vec<Vec<String>> {
        t.iter()
            .map(|r| r.iter().map(|c| (*c).to_string()).collect())
            .collect()
    }

    fn read(t: &[&[&str]], profile: &Profile) -> Parsed {
        let rows = rows(t);
        let h = find_header(&rows, profile).expect("header");
        let mapping = map_columns(&rows[h], profile);
        parse(&rows, h, &mapping, profile, Currency::Jpy)
    }

    #[test]
    fn japanese_and_english_headers_are_detected() {
        let ja = rows(&[&[
            "請求書番号",
            "発注No.",
            "請求日",
            "品番",
            "数量",
            "単価（円）",
            "消費税額",
        ]]);
        let p = Profile::detect("x", ImportKind::Invoice);
        let m = map_columns(&ja[0], &p);
        assert_eq!(
            m,
            vec![
                Some(Field::InvoiceNumber),
                Some(Field::PoNumber),
                Some(Field::InvoiceDate),
                Some(Field::Mpn),
                Some(Field::Quantity),
                Some(Field::UnitPrice),
                Some(Field::TaxAmount),
            ]
        );
        let en = rows(&[&[
            "Invoice No",
            "PO Number",
            "Invoice Date",
            "Part No",
            "Qty",
            "Amount",
            "Tax",
        ]]);
        assert!(missing_fields(ImportKind::Invoice, &map_columns(&en[0], &p)).is_empty());
        // A title block before the header is skipped; a file without the required columns has none.
        let t = rows(&[&["請求明細", ""], &["請求番号", "発注番号"]]);
        assert_eq!(find_header(&t, &p), None);
        assert_eq!(
            missing_fields(ImportKind::Invoice, &map_columns(&t[1], &p)),
            vec![
                Field::InvoiceDate,
                Field::Quantity,
                Field::TaxAmount,
                Field::Mpn,
                Field::UnitPrice
            ]
        );
    }

    #[test]
    fn explicit_columns_win_over_synonyms() {
        let mut p = Profile::detect("erp_x", ImportKind::Receipt);
        p.columns = vec![
            ("伝票番号".into(), Field::PoNumber),
            ("日付".into(), Field::ReceivedDate),
            ("コード".into(), Field::Mpn),
        ];
        p.validate().unwrap();
        let h = rows(&[&["伝票番号", "発注番号", "日付", "コード", "数量"]]);
        assert_eq!(
            map_columns(&h[0], &p),
            vec![
                Some(Field::PoNumber),
                None, // already taken by the explicit column
                Some(Field::ReceivedDate),
                Some(Field::Mpn),
                Some(Field::Quantity),
            ]
        );
    }

    #[test]
    fn profiles_are_validated() {
        let ok = Profile::detect("generic", ImportKind::Po);
        assert_eq!(ok.validate(), Ok(()));
        let bad = |f: &dyn Fn(&mut Profile)| {
            let mut p = ok.clone();
            f(&mut p);
            p.validate().unwrap_err()
        };
        assert_eq!(bad(&|p| p.name = "Bad Name".into()), ProfileError::Name);
        assert_eq!(
            bad(&|p| p.columns = vec![("a".into(), Field::Mpn), ("b".into(), Field::Mpn)]),
            ProfileError::DuplicateField
        );
        assert_eq!(
            bad(&|p| p.columns = vec![("()".into(), Field::Mpn)]),
            ProfileError::ColumnName
        );
        assert_eq!(
            bad(&|p| p.thousands_separator = Some('.')),
            ProfileError::Separators
        );
        assert_eq!(
            bad(&|p| p.default_tax_rate_bps = Some(10_001)),
            ProfileError::TaxRate
        );
        for p in presets() {
            assert_eq!(p.validate(), Ok(()), "{}", p.name);
            assert!(p.name.starts_with("generic_"));
        }
        assert_eq!(
            preset("generic_ja_invoice").unwrap().kind,
            ImportKind::Invoice
        );
    }

    #[test]
    fn numbers_follow_the_profile_separators() {
        let ja = Profile::detect("x", ImportKind::Invoice);
        assert_eq!(parse_number("¥1,234", 0, &ja), Some(1234));
        assert_eq!(parse_number("１，２３４円", 0, &ja), Some(1234));
        assert_eq!(parse_number("12.5", 2, &ja), Some(1250));
        assert_eq!(parse_number("12.555", 2, &ja), None);
        assert_eq!(parse_number("-5", 0, &ja), None);
        assert_eq!(parse_number("1.2.3", 2, &ja), None);
        assert_eq!(parse_number("abc", 0, &ja), None);
        let eu = Profile {
            decimal_separator: ',',
            thousands_separator: Some('.'),
            ..ja.clone()
        };
        assert_eq!(parse_number("1.234,50", 2, &eu), Some(123_450));
        assert_eq!(parse_number("1,5", 0, &eu), None);
        assert_eq!(parse_tax_rate("10%", &ja), Some(1000));
        assert_eq!(parse_tax_rate("8", &ja), Some(800));
        assert_eq!(parse_tax_rate("0.08", &ja), Some(800));
        assert_eq!(parse_tax_rate("150%", &ja), None);
        assert_eq!(
            parse_day("05/01/2026", DateFormat::Dmy),
            Day::new(2026, 1, 5)
        );
        assert_eq!(
            parse_day("01/05/2026", DateFormat::Mdy),
            Day::new(2026, 1, 5)
        );
        assert_eq!(
            parse_day("令和8年1月5日", DateFormat::Auto),
            Day::new(2026, 1, 5)
        );
        assert_eq!(parse_day("31/02/2026", DateFormat::Dmy), None);
    }

    /// L9 C2: an invoice without a rate is refused when the profile has no default rate,
    /// and the profile's rounding rule is what the record carries.
    #[test]
    fn invoice_tax_rate_and_rounding_come_from_the_profile() {
        let rows: &[&[&str]] = &[
            &[
                "請求番号",
                "発注番号",
                "請求日",
                "品番",
                "数量",
                "単価",
                "消費税",
            ],
            &["INV-9", "PO-9", "2026/10/20", "6204ZZ", "3", "333", "99"],
        ];
        let mut p = preset("generic_ja_invoice").unwrap();
        p.default_tax_rate_bps = None;
        let parsed = read(rows, &p);
        assert!(parsed.records.is_empty());
        assert_eq!(
            parsed.errors,
            vec![RowError {
                row: 2,
                field: Some(Field::TaxRate),
                code: ErrorCode::Required,
            }]
        );
        p.default_tax_rate_bps = Some(800);
        p.tax_rounding = TaxRounding::HalfUp;
        let parsed = read(rows, &p);
        let Record::Invoice(inv) = &parsed.records[0] else {
            panic!("{:?}", parsed.errors)
        };
        assert_eq!(
            (inv.tax_rate_bps, inv.tax_rounding),
            (800, TaxRounding::HalfUp)
        );
    }

    #[test]
    fn invoice_rows_become_records_with_minor_units() {
        let p = preset("generic_ja_invoice").unwrap();
        let parsed = read(
            &[
                &["請求書一覧"],
                &[
                    "請求番号",
                    "発注番号",
                    "請求日",
                    "品番",
                    "数量",
                    "単価",
                    "金額",
                    "消費税",
                    "送料",
                ],
                &[
                    "INV-1",
                    "PO-1",
                    "2026/10/20",
                    "6204ZZ",
                    "100",
                    "1,200",
                    "120,000",
                    "30,200",
                    "2,000",
                ],
                &[
                    "INV-1",
                    "PO-1",
                    "2026/10/20",
                    "HSR15",
                    "4",
                    "45,000",
                    "",
                    "30,200",
                    "2,000",
                ],
                &["", "", "", "", "", "", "", "", ""],
                &[
                    "INV-2",
                    "PO-2",
                    "2026-10-21",
                    "X1",
                    "3",
                    "",
                    "100",
                    "10",
                    "",
                ],
            ],
            &p,
        );
        assert_eq!(parsed.data_rows, 3);
        assert_eq!(parsed.records.len(), 1, "{:?}", parsed.errors);
        let Record::Invoice(inv) = &parsed.records[0] else {
            panic!()
        };
        assert_eq!(inv.invoice_number, "INV-1");
        assert_eq!(inv.tax_minor, 30_200);
        assert_eq!(inv.shipping_minor, 2000);
        assert_eq!(inv.tax_rate_bps, 1000);
        assert_eq!(inv.rows, vec![3, 4]);
        assert_eq!(inv.lines[1].unit_price_minor, Some(45_000));
        // 100 / 3 is not a whole yen: refused, not rounded.
        assert_eq!(
            parsed.errors,
            vec![RowError {
                row: 6,
                field: Some(Field::LineAmount),
                code: ErrorCode::AmountInvalid
            }]
        );
    }

    #[test]
    fn a_bad_row_holds_back_its_whole_record() {
        let p = Profile::detect("x", ImportKind::Receipt);
        let parsed = read(
            &[
                &["発注番号", "納品日", "品番", "数量"],
                &["PO-1", "2026/10/01", "A", "10"],
                &["PO-1", "2026/10/01", "B", "ten"],
                &["PO-2", "2026/10/01", "C", "1"],
                &["PO-3", "2026/13/01", "D", "1"],
                &["PO-3", "2026/10/02", "E", "1"],
                &["", "2026/10/02", "F", "1"],
            ],
            &p,
        );
        let pos: Vec<&str> = parsed.records.iter().map(Record::po_number).collect();
        assert_eq!(pos, vec!["PO-2"]);
        let codes: Vec<(usize, Option<Field>, &str)> = parsed
            .errors
            .iter()
            .map(|e| (e.row, e.field, e.code.code()))
            .collect();
        assert_eq!(
            codes,
            vec![
                (3, Some(Field::Quantity), "quantity_invalid"),
                (5, Some(Field::ReceivedDate), "date_invalid"),
                (7, Some(Field::PoNumber), "required"),
            ]
        );
    }

    #[test]
    fn header_values_must_agree_within_a_record() {
        let p = Profile::detect("x", ImportKind::Invoice);
        let parsed = read(
            &[
                &[
                    "請求番号",
                    "発注番号",
                    "請求日",
                    "品番",
                    "数量",
                    "単価",
                    "消費税",
                    "税率",
                ],
                &["I1", "PO-1", "2026/10/20", "A", "1", "100", "10", "10%"],
                &["I1", "PO-1", "2026/10/21", "B", "1", "100", "10", "10%"],
                &["I2", "PO-2", "2026/10/20", "A", "1", "100", "10", "8%"],
                &["I2", "PO-2", "2026/10/20", "B", "1", "100", "", "8%"],
            ],
            &p,
        );
        assert_eq!(parsed.records.len(), 1);
        let Record::Invoice(i2) = &parsed.records[0] else {
            panic!()
        };
        assert_eq!(i2.tax_rate_bps, 800);
        assert_eq!(parsed.errors[0].code, ErrorCode::Inconsistent);
        assert_eq!(parsed.errors[0].row, 3);
    }

    #[test]
    fn purchase_orders_need_po_and_request_numbers() {
        let p = preset("generic_en_po").unwrap();
        let parsed = read(
            &[
                &[
                    "PO Number",
                    "Request Number",
                    "Part Number",
                    "Quantity",
                    "Unit Price",
                ],
                &["PO-9", "PR-000001", "6204ZZ", "100", "1200"],
                &["PO-9", "PR-000001", "HSR15", "4", "45000"],
                &["PO-10", "", "X", "1", "1"],
            ],
            &p,
        );
        assert_eq!(parsed.records.len(), 1);
        let Record::Po(po) = &parsed.records[0] else {
            panic!()
        };
        assert_eq!(po.request_number, "PR-000001");
        assert_eq!(po.lines.len(), 2);
        assert_eq!(parsed.errors[0].field, Some(Field::RequestNumber));
    }

    #[test]
    fn too_many_rows_are_refused_whole() {
        let p = Profile::detect("x", ImportKind::Receipt);
        let mut t = vec![vec![
            "発注番号".to_string(),
            "納品日".into(),
            "品番".into(),
            "数量".into(),
        ]];
        for i in 0..=MAX_DATA_ROWS {
            t.push(vec![
                format!("PO-{i}"),
                "2026/1/1".into(),
                "A".into(),
                "1".into(),
            ]);
        }
        let mapping = map_columns(&t[0], &p);
        let parsed = parse(&t, 0, &mapping, &p, Currency::Jpy);
        assert!(parsed.records.is_empty());
        assert_eq!(parsed.errors[0].code, ErrorCode::TooManyRows);
    }

    #[test]
    fn lines_are_matched_by_number_part_or_description() {
        let known = vec![
            KnownLine {
                line_no: 1,
                mpns: vec!["6204ZZ".into(), "VERIFIED-0".into()],
                description: "Deep groove ball bearing".into(),
            },
            KnownLine {
                line_no: 2,
                mpns: vec!["HSR15".into()],
                description: "Linear guide".into(),
            },
        ];
        let line = |n: Option<u32>, mpn: &str, d: &str| RecordLine {
            row: 1,
            line_no: n,
            mpn: mpn.into(),
            description: d.into(),
            quantity: Some(1),
            unit_price_minor: None,
        };
        assert_eq!(match_line(&known, &line(Some(2), "", "")), Some(1));
        assert_eq!(match_line(&known, &line(Some(3), "6204ZZ", "")), None);
        assert_eq!(
            match_line(&known, &line(None, "ｖｅｒｉｆｉｅｄ-0", "")),
            Some(0)
        );
        assert_eq!(
            match_line(&known, &line(None, "", "linear  GUIDE")),
            Some(1)
        );
        assert_eq!(
            match_line(&known, &line(None, "NOPE", "Linear guide")),
            None
        );
    }
}
