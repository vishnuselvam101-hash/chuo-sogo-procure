//! Connectors to the factory's own systems (docs/agents/INTEGRATIONS.md). A [`Connector`]
//! pulls ERP-neutral records (`procure_domain::connector`) from a source and pushes exports
//! to a target. Only the file connector exists: CSV / TSV / xlsx exports exchanged by hand.
//! Adapters for ERP APIs implement the same trait later; whatever they pull goes through the
//! same staging, preview, cross-check and apply as a file, never straight into orders.

use chrono::NaiveDate;
use futures_util::future::BoxFuture;
use procure_domain::connector::{
    self as conn, DateFormat, Field, ImportKind, Profile, Record, RowError, TextEncoding,
};
use procure_domain::history::Day;
use procure_domain::money::Currency;
use serde::{Deserialize, Serialize};

use crate::api::exports;
use crate::intake::{self, IntakeError};

/// Largest file one import reads (smaller than attachments: tables only).
pub const MAX_IMPORT_BYTES: usize = 10 * 1024 * 1024;

/// What a source is asked for.
pub struct PullRequest {
    pub file_name: String,
    pub bytes: Vec<u8>,
    /// For records whose file has no currency column.
    pub currency: Currency,
}

/// Records read from a source, with the problems of the rows that could not be read.
pub struct Pulled {
    pub records: Vec<Record>,
    pub errors: Vec<RowError>,
    pub data_rows: usize,
    /// Field of each source column (`None` = ignored).
    pub mapping: Vec<Option<Field>>,
}

/// A table to hand to the factory's systems.
pub struct Export {
    pub file_name: String,
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ConnectorError {
    /// Not a table this connector reads (by name or content).
    Unsupported,
    FileSize,
    /// The bytes are not in the profile's encoding.
    Encoding,
    /// No header row with the fields the kind needs (the missing ones).
    HeaderNotFound(Vec<Field>),
    Intake(IntakeError),
}

impl ConnectorError {
    pub fn code(&self) -> &'static str {
        match self {
            ConnectorError::Unsupported => "unsupported",
            ConnectorError::FileSize => "file_size",
            ConnectorError::Encoding => "encoding",
            ConnectorError::HeaderNotFound(_) => "header_not_found",
            ConnectorError::Intake(e) => e.code(),
        }
    }
}

/// A source of records and target of exports (file today; ERP APIs later).
pub trait Connector: Send + Sync {
    /// Stable name (`file`, later e.g. an ERP adapter's).
    fn name(&self) -> &'static str;

    /// Reads the records of `profile.kind`. Must not write anything anywhere.
    fn pull<'a>(
        &'a self,
        req: &'a PullRequest,
        profile: &'a Profile,
    ) -> BoxFuture<'a, Result<Pulled, ConnectorError>>;

    /// Delivers an export; returns what was delivered (the file connector: the file bytes).
    fn push<'a>(&'a self, export: &'a Export) -> BoxFuture<'a, Result<Vec<u8>, ConnectorError>>;
}

/// CSV / TSV (UTF-8 or Shift_JIS) and xlsx files.
pub struct FileConnector;

fn extension(file_name: &str) -> String {
    file_name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// The table as UTF-8 for the intake reader, in the profile's encoding.
fn transcode(bytes: &[u8], encoding: TextEncoding) -> Result<Vec<u8>, ConnectorError> {
    match encoding {
        TextEncoding::Auto => Ok(bytes.to_vec()),
        TextEncoding::Utf8 => std::str::from_utf8(bytes)
            .map(|_| bytes.to_vec())
            .map_err(|_| ConnectorError::Encoding),
        TextEncoding::ShiftJis => {
            let (text, _, had_errors) = encoding_rs::SHIFT_JIS.decode(bytes);
            if had_errors {
                return Err(ConnectorError::Encoding);
            }
            Ok(text.into_owned().into_bytes())
        }
    }
}

impl Connector for FileConnector {
    fn name(&self) -> &'static str {
        "file"
    }

    fn pull<'a>(
        &'a self,
        req: &'a PullRequest,
        profile: &'a Profile,
    ) -> BoxFuture<'a, Result<Pulled, ConnectorError>> {
        Box::pin(async move {
            let ext = extension(&req.file_name);
            if !matches!(ext.as_str(), "csv" | "tsv" | "txt" | "xlsx") {
                return Err(ConnectorError::Unsupported);
            }
            if req.bytes.is_empty() || req.bytes.len() > MAX_IMPORT_BYTES {
                return Err(ConnectorError::FileSize);
            }
            // Exact tables only: a PDF, an image or an executable renamed `.csv` is refused
            // (scans go through the AI intake, which a person confirms).
            let is_zip = req.bytes.starts_with(b"PK\x03\x04");
            if req.bytes.starts_with(b"%PDF-")
                || intake::image_kind(&req.bytes).is_some()
                || req.bytes.starts_with(b"MZ")
                || req.bytes.starts_with(b"\x7fELF")
                || (ext == "xlsx") != is_zip
            {
                return Err(ConnectorError::Unsupported);
            }
            let bytes = if ext == "xlsx" {
                req.bytes.clone()
            } else {
                transcode(&req.bytes, profile.encoding)?
            };
            let table = intake::table_rows(&req.file_name, &bytes)
                .await
                .map_err(ConnectorError::Intake)?;
            let Some(header) = conn::find_header(&table.rows, profile) else {
                let first = table.rows.first().cloned().unwrap_or_default();
                return Err(ConnectorError::HeaderNotFound(conn::missing_fields(
                    profile.kind,
                    &conn::map_columns(&first, profile),
                )));
            };
            let mapping = conn::map_columns(&table.rows[header], profile);
            let parsed = conn::parse(&table.rows, header, &mapping, profile, req.currency);
            Ok(Pulled {
                records: parsed.records,
                errors: parsed.errors,
                data_rows: parsed.data_rows,
                mapping,
            })
        })
    }

    fn push<'a>(&'a self, export: &'a Export) -> BoxFuture<'a, Result<Vec<u8>, ConnectorError>> {
        Box::pin(async move { Ok(render_csv(&export.header, &export.rows).into_bytes()) })
    }
}

/// UTF-8 CSV with a byte-order mark (Excel reads Japanese correctly); every cell through
/// [`exports::cell`], so text a spreadsheet would run as a formula (`=`, `+`, `-`, `@`) is
/// quoted with `'`.
pub fn render_csv(header: &[String], rows: &[Vec<String>]) -> String {
    let mut out = String::from("\u{feff}");
    for r in std::iter::once(header).chain(rows.iter().map(Vec::as_slice)) {
        let cells: Vec<String> = r.iter().map(|c| exports::cell(c)).collect();
        out.push_str(&cells.join(","));
        out.push_str("\r\n");
    }
    out
}

/// A profile as stored and sent over the API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct ProfileConfig {
    /// `po` | `receipt` | `invoice`
    pub kind: String,
    /// Source column header → field code. Columns not listed are recognised by header.
    #[serde(default)]
    pub columns: Vec<ColumnMap>,
    /// `auto` (default) | `ymd` | `dmy` | `mdy`
    #[serde(default = "auto")]
    pub date_format: String,
    /// `.` (default) or `,`
    #[serde(default = "dot")]
    pub decimal_separator: String,
    /// `,` (default), `.`, ` `, `'` or empty for none
    #[serde(default = "comma")]
    pub thousands_separator: String,
    /// `auto` (default: UTF-8, else Shift_JIS) | `utf-8` | `shift_jis`
    #[serde(default = "auto")]
    pub encoding: String,
    /// When the file has no tax-rate column (basis points; default 1000 = 10%).
    #[serde(default = "ten_percent")]
    pub default_tax_rate_bps: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct ColumnMap {
    pub source: String,
    /// A field code (`po_number`, `request_number`, `order_date`, `received_date`,
    /// `invoice_number`, `invoice_date`, `due_date`, `line_no`, `mpn`, `description`,
    /// `quantity`, `unit_price`, `line_amount`, `currency`, `tax_rate`, `tax_amount`,
    /// `shipping`, `note`).
    pub field: String,
}

fn auto() -> String {
    "auto".into()
}
fn dot() -> String {
    ".".into()
}
fn comma() -> String {
    ",".into()
}
fn ten_percent() -> u32 {
    1000
}

fn one_char(s: &str) -> Option<char> {
    let mut it = s.chars();
    let c = it.next()?;
    it.next().is_none().then_some(c)
}

impl ProfileConfig {
    /// The domain profile, validated; the error is a field code for the API.
    pub fn to_profile(&self, name: &str) -> Result<Profile, &'static str> {
        let kind = ImportKind::from_code(&self.kind).ok_or("kind")?;
        let columns = self
            .columns
            .iter()
            .map(|c| {
                Ok((
                    c.source.clone(),
                    Field::from_code(&c.field).ok_or("columns")?,
                ))
            })
            .collect::<Result<Vec<_>, &'static str>>()?;
        let thousands = match self.thousands_separator.as_str() {
            "" => None,
            s => Some(one_char(s).ok_or("thousands_separator")?),
        };
        let p = Profile {
            name: name.to_string(),
            kind,
            columns,
            date_format: DateFormat::from_code(&self.date_format).ok_or("date_format")?,
            decimal_separator: one_char(&self.decimal_separator).ok_or("decimal_separator")?,
            thousands_separator: thousands,
            encoding: TextEncoding::from_code(&self.encoding).ok_or("encoding")?,
            default_tax_rate_bps: self.default_tax_rate_bps,
        };
        p.validate().map_err(|e| e.code())?;
        Ok(p)
    }

    pub fn from_profile(p: &Profile) -> Self {
        ProfileConfig {
            kind: p.kind.code().into(),
            columns: p
                .columns
                .iter()
                .map(|(s, f)| ColumnMap {
                    source: s.clone(),
                    field: f.code().into(),
                })
                .collect(),
            date_format: p.date_format.code().into(),
            decimal_separator: p.decimal_separator.to_string(),
            thousands_separator: p.thousands_separator.map(String::from).unwrap_or_default(),
            encoding: p.encoding.code().into(),
            default_tax_rate_bps: p.default_tax_rate_bps,
        }
    }
}

/// One record as staged in `import_rows.record`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct Staged {
    /// `po` | `receipt` | `invoice`
    pub kind: String,
    pub po_number: String,
    /// Purchase orders: the platform request (`PR-…`) the factory's PO is for.
    #[serde(default)]
    pub request_number: String,
    /// Invoices: the supplier's invoice number.
    #[serde(default)]
    pub invoice_number: String,
    /// Order, receipt or invoice date.
    pub date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub currency: String,
    pub tax_rate_bps: Option<u32>,
    pub tax_minor: Option<i64>,
    pub shipping_minor: Option<i64>,
    #[serde(default)]
    pub note: String,
    pub lines: Vec<StagedLine>,
    pub rows: Vec<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct StagedLine {
    pub row: i32,
    pub line_no: Option<u32>,
    pub mpn: String,
    pub description: String,
    pub quantity: Option<u32>,
    pub unit_price_minor: Option<i64>,
}

fn date(d: Day) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(d.year, d.month, d.day)
}

fn row(r: usize) -> i32 {
    i32::try_from(r).unwrap_or(i32::MAX)
}

impl Staged {
    pub fn from_record(r: &Record) -> Self {
        let lines = r
            .lines()
            .iter()
            .map(|l| StagedLine {
                row: row(l.row),
                line_no: l.line_no,
                mpn: l.mpn.clone(),
                description: l.description.clone(),
                quantity: l.quantity,
                unit_price_minor: l.unit_price_minor,
            })
            .collect();
        let rows = r.rows().iter().map(|r| row(*r)).collect();
        let base = Staged {
            kind: String::new(),
            po_number: r.po_number().to_string(),
            request_number: String::new(),
            invoice_number: String::new(),
            date: None,
            due_date: None,
            currency: String::new(),
            tax_rate_bps: None,
            tax_minor: None,
            shipping_minor: None,
            note: String::new(),
            lines,
            rows,
        };
        match r {
            Record::Po(p) => Staged {
                kind: ImportKind::Po.code().into(),
                request_number: p.request_number.clone(),
                date: p.order_date.and_then(date),
                currency: p.currency.code().into(),
                ..base
            },
            Record::Receipt(p) => Staged {
                kind: ImportKind::Receipt.code().into(),
                date: date(p.received_on),
                note: p.note.clone(),
                ..base
            },
            Record::Invoice(p) => Staged {
                kind: ImportKind::Invoice.code().into(),
                invoice_number: p.invoice_number.clone(),
                date: date(p.invoice_date),
                due_date: p.due_date.and_then(date),
                currency: p.currency.code().into(),
                tax_rate_bps: Some(p.tax_rate_bps),
                tax_minor: Some(p.tax_minor),
                shipping_minor: Some(p.shipping_minor),
                ..base
            },
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;

    #[tokio::test]
    async fn the_file_connector_refuses_what_is_not_a_table() {
        let p = Profile::detect("x", ImportKind::Receipt);
        let pull = |name: &str, bytes: &[u8]| PullRequest {
            file_name: name.into(),
            bytes: bytes.to_vec(),
            currency: Currency::Jpy,
        };
        for (name, bytes) in [
            ("a.pdf", b"%PDF-1.4".as_slice()),
            ("a.csv", b"%PDF-1.4 x"),
            ("a.csv", b"MZ\x90\x00"),
            ("a.csv", b"PK\x03\x04zip"),
            ("a.xlsx", b"not a zip"),
            ("a.exe", b"a,b"),
        ] {
            let e = FileConnector.pull(&pull(name, bytes), &p).await.err();
            assert_eq!(e, Some(ConnectorError::Unsupported), "{name}");
        }
        let big = vec![b'a'; MAX_IMPORT_BYTES + 1];
        assert_eq!(
            FileConnector.pull(&pull("a.csv", &big), &p).await.err(),
            Some(ConnectorError::FileSize)
        );
        let e = FileConnector
            .pull(&pull("a.csv", "品名,数量\nx,1\n".as_bytes()), &p)
            .await
            .err();
        assert_eq!(
            e,
            Some(ConnectorError::HeaderNotFound(vec![
                Field::PoNumber,
                Field::ReceivedDate
            ]))
        );
    }

    #[tokio::test]
    async fn shift_jis_files_are_read_in_either_encoding_setting() {
        let csv = "発注番号,納品日,品番,数量\r\nPO-1,2026/10/1,6204ZZ,100\r\n";
        let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode(csv);
        for enc in [TextEncoding::Auto, TextEncoding::ShiftJis] {
            let p = Profile {
                encoding: enc,
                ..Profile::detect("x", ImportKind::Receipt)
            };
            let req = PullRequest {
                file_name: "r.csv".into(),
                bytes: sjis.to_vec(),
                currency: Currency::Jpy,
            };
            let pulled = FileConnector.pull(&req, &p).await.unwrap();
            assert_eq!(pulled.records.len(), 1);
            assert_eq!(pulled.records[0].po_number(), "PO-1");
        }
        let p = Profile {
            encoding: TextEncoding::Utf8,
            ..Profile::detect("x", ImportKind::Receipt)
        };
        let req = PullRequest {
            file_name: "r.csv".into(),
            bytes: sjis.to_vec(),
            currency: Currency::Jpy,
        };
        assert_eq!(
            FileConnector.pull(&req, &p).await.err(),
            Some(ConnectorError::Encoding)
        );
    }

    /// A minimal workbook (inline strings) with one sheet holding `rows`.
    fn xlsx(rows: &[&[&str]]) -> Vec<u8> {
        use std::io::Write;
        fn esc(s: &str) -> String {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        }
        let mut sheet = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
        );
        for (r, row) in rows.iter().enumerate() {
            sheet.push_str(&format!(r#"<row r="{}">"#, r + 1));
            for (c, v) in row.iter().enumerate() {
                let col = char::from(b'A' + u8::try_from(c).unwrap());
                sheet.push_str(&format!(
                    r#"<c r="{col}{}" t="inlineStr"><is><t>{}</t></is></c>"#,
                    r + 1,
                    esc(v)
                ));
            }
            sheet.push_str("</row>");
        }
        sheet.push_str("</sheetData></worksheet>");
        let files = [
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#.to_string(),
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string(),
            ),
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="発注" sheetId="1" r:id="rId1"/></sheets></workbook>"#.to_string(),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#.to_string(),
            ),
            ("xl/worksheets/sheet1.xml", sheet),
        ];
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut out);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, body) in files {
                z.start_file(name, opts).unwrap();
                z.write_all(body.as_bytes()).unwrap();
            }
            z.finish().unwrap();
        }
        out.into_inner()
    }

    #[tokio::test]
    async fn xlsx_exports_are_read_like_csv() {
        let bytes = xlsx(&[
            &["発注一覧"],
            &["発注番号", "依頼番号", "品番", "数量", "単価"],
            &["PO-1", "PR-000001", "6204ZZ", "100", "1200"],
            &["PO-1", "PR-000001", "HSR15", "4", "45000"],
        ]);
        let req = PullRequest {
            file_name: "発注.xlsx".into(),
            bytes,
            currency: Currency::Jpy,
        };
        let p = conn::preset("generic_ja_po").unwrap();
        let pulled = FileConnector.pull(&req, &p).await.unwrap();
        assert!(pulled.errors.is_empty(), "{:?}", pulled.errors);
        let Record::Po(po) = &pulled.records[0] else {
            panic!("not a PO")
        };
        assert_eq!(po.request_number, "PR-000001");
        assert_eq!(po.lines[1].unit_price_minor, Some(45_000));
        assert_eq!(po.rows, vec![3, 4]);
    }

    #[tokio::test]
    async fn pushed_csv_quotes_formula_cells() {
        let out = FileConnector
            .push(&Export {
                file_name: "x.csv".into(),
                header: vec!["a".into(), "b".into()],
                rows: vec![vec!["=HYPERLINK(\"x\")".into(), "+1".into()]],
            })
            .await
            .unwrap();
        let s = String::from_utf8(out).unwrap();
        assert_eq!(s, "\u{feff}a,b\r\n\"'=HYPERLINK(\"\"x\"\")\",'+1\r\n");
    }

    #[test]
    fn profile_configs_round_trip() {
        for p in conn::presets() {
            let c = ProfileConfig::from_profile(&p);
            assert_eq!(c.to_profile(&p.name).unwrap(), p);
        }
        let mut c = ProfileConfig::from_profile(&Profile::detect("x", ImportKind::Po));
        c.columns.push(ColumnMap {
            source: "a".into(),
            field: "nope".into(),
        });
        assert_eq!(c.to_profile("x"), Err("columns"));
        c.columns.clear();
        c.decimal_separator = ",".into();
        assert_eq!(c.to_profile("x"), Err("profile_separators"));
    }
}
