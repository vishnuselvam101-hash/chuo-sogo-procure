//! PDF rendering of issued invoices and credit notes (A4, Japanese/English labels).
//!
//! Only presentation: every amount printed here was computed by `procure_domain::invoicing`
//! and stored before rendering. The font (a CJK font with Japanese glyphs) is read from disk
//! once at start-up; without it, documents cannot be issued.

use std::sync::Arc;

use krilla::Document;
use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Rect};
use krilla::metadata::Metadata;
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, Stroke};
use krilla::text::{Font, TextDirection};
use procure_domain::money::{Currency, Money};
use skrifa::MetadataProvider;
use skrifa::instance::{LocationRef, Size};

/// Default: Noto Sans CJK (Debian/Ubuntu `fonts-noto-cjk`), face 0 = Japanese.
pub const DEFAULT_FONT_PATH: &str = "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc";

#[derive(Clone)]
pub struct PdfFont {
    data: Arc<Vec<u8>>,
    index: u32,
}

impl PdfFont {
    /// Loads and checks the font (must contain the glyphs this module prints).
    pub fn load(path: &str, index: u32) -> Result<Self, String> {
        let data = std::fs::read(path).map_err(|e| format!("cannot read PDF font {path}: {e}"))?;
        let font = PdfFont {
            data: Arc::new(data),
            index,
        };
        let face = skrifa::FontRef::from_index(&font.data, index)
            .map_err(|e| format!("invalid PDF font {path}: {e}"))?;
        let cmap = face.charmap();
        if "請求書御中登録番号消費税軽減税率¥0123456789ABCabc"
            .chars()
            .any(|c| cmap.map(c).is_none())
        {
            return Err(format!("PDF font {path} lacks Japanese glyphs"));
        }
        Ok(font)
    }

    fn width(&self, text: &str, size: f32) -> f32 {
        let Ok(face) = skrifa::FontRef::from_index(&self.data, self.index) else {
            return 0.0;
        };
        let cmap = face.charmap();
        let metrics = face.glyph_metrics(Size::new(size), LocationRef::default());
        text.chars()
            .map(|c| {
                cmap.map(c)
                    .and_then(|g| metrics.advance_width(g))
                    .unwrap_or(size * 0.5)
            })
            .sum()
    }
}

pub struct Party {
    pub name: String,
    pub address: String,
}

pub struct Issuer {
    pub legal_name: String,
    pub registration_number: String,
    pub address: String,
    pub phone: String,
    pub email: String,
    pub bank_details: String,
}

pub struct PdfLine {
    pub description: String,
    pub quantity: u32,
    pub unit: String,
    pub unit_price: Money,
    pub tax_rate_bps: u32,
    pub amount: Money,
}

pub struct PdfRate {
    pub rate_bps: u32,
    pub base: Money,
    pub tax: Money,
}

pub struct PdfDocument {
    pub credit_note: bool,
    /// Unit prices and line amounts include tax (consumer sales); per-rate totals are shown
    /// tax-inclusive with the tax contained.
    pub tax_inclusive: bool,
    pub number: String,
    pub issue_date: String,
    pub due_date: Option<String>,
    /// Credit notes: the original invoice's number and issue date.
    pub original: Option<(String, String)>,
    pub recipient: Party,
    pub issuer: Issuer,
    pub lines: Vec<PdfLine>,
    pub per_rate: Vec<PdfRate>,
    pub subtotal: Money,
    pub tax: Money,
    pub total: Money,
    pub notes: String,
    pub reason: String,
}

/// Money as printed: `¥1,234` for yen, `USD 1,234.50` otherwise.
pub fn money(m: Money) -> String {
    let digits = m.currency().minor_digits();
    let neg = m.minor() < 0;
    let abs = m.minor().unsigned_abs();
    let scale = 10u64.pow(digits);
    let whole = (abs / scale).to_string();
    let mut grouped = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    let frac = if digits > 0 {
        format!(".{:0width$}", abs % scale, width = digits as usize)
    } else {
        String::new()
    };
    let sign = if neg { "-" } else { "" };
    match m.currency() {
        Currency::Jpy => format!("{sign}¥{grouped}{frac}"),
        c => format!("{sign}{} {grouped}{frac}", c.code()),
    }
}

fn rate_label(bps: u32) -> String {
    match bps {
        0 => "非課税 0%".into(),
        800 => "8%※".into(),
        b => format!("{}%", b / 100),
    }
}

const PAGE_W: f32 = 595.28;
const PAGE_H: f32 = 841.89;
const MARGIN: f32 = 42.0;
const ROW_H: f32 = 15.0;

struct Pen<'a> {
    font: &'a PdfFont,
    kfont: Font,
}

impl Pen<'_> {
    fn text(&self, s: &mut krilla::surface::Surface<'_>, x: f32, y: f32, size: f32, t: &str) {
        if t.is_empty() {
            return;
        }
        s.draw_text(
            Point::from_xy(x, y),
            self.kfont.clone(),
            size,
            t,
            false,
            TextDirection::Auto,
        );
    }
    fn right(&self, s: &mut krilla::surface::Surface<'_>, x: f32, y: f32, size: f32, t: &str) {
        let w = self.font.width(t, size);
        self.text(s, x - w, y, size, t);
    }
    /// Breaks `t` into lines no wider than `width` (character based: works for Japanese,
    /// prefers spaces for Latin text).
    fn wrap(&self, t: &str, size: f32, width: f32) -> Vec<String> {
        let mut out = Vec::new();
        for para in t.lines() {
            let mut line = String::new();
            for c in para.chars() {
                let mut candidate = line.clone();
                candidate.push(c);
                if self.font.width(&candidate, size) > width && !line.is_empty() {
                    let cut = line
                        .rfind(' ')
                        .filter(|&i| i > line.len() / 2)
                        .unwrap_or(line.len());
                    let rest = line[cut..].trim_start().to_string();
                    out.push(line[..cut].trim_end().to_string());
                    line = rest;
                }
                line.push(c);
            }
            out.push(line);
        }
        if out.is_empty() {
            out.push(String::new());
        }
        out
    }
}

fn rule(s: &mut krilla::surface::Surface<'_>, x1: f32, y: f32, x2: f32, width: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(x1, y);
    pb.line_to(x2, y);
    if let Some(path) = pb.finish() {
        s.set_fill(None);
        s.set_stroke(Some(Stroke {
            paint: rgb::Color::new(60, 60, 60).into(),
            width,
            ..Default::default()
        }));
        s.draw_path(&path);
        s.set_stroke(None);
        s.set_fill(Some(Fill::default()));
    }
}

fn shade(s: &mut krilla::surface::Surface<'_>, x: f32, y: f32, w: f32, h: f32) {
    let mut pb = PathBuilder::new();
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        pb.push_rect(r);
    }
    if let Some(path) = pb.finish() {
        s.set_fill(Some(Fill {
            paint: rgb::Color::new(235, 239, 245).into(),
            opacity: NormalizedF32::ONE,
            rule: Default::default(),
        }));
        s.draw_path(&path);
        s.set_fill(Some(Fill::default()));
    }
}

// Table columns (x positions; amounts are right-aligned at the column's right edge).
const C_NO: f32 = MARGIN;
const C_DESC: f32 = MARGIN + 24.0;
const C_QTY_R: f32 = 360.0;
const C_UNIT_R: f32 = 440.0;
const C_RATE_R: f32 = 482.0;
const C_AMT_R: f32 = PAGE_W - MARGIN;
const DESC_W: f32 = 250.0;

#[derive(Debug)]
pub enum RenderError {
    /// Reason, notes and bank details together do not fit on one page: refused rather than
    /// printing a cut-off text on a document that can never be changed.
    TextTooLong,
    Other(String),
}

impl From<&str> for RenderError {
    fn from(e: &str) -> Self {
        RenderError::Other(e.into())
    }
}

/// Renders the document. Pure function of its input (no clock, no randomness).
pub fn render(font: &PdfFont, doc: &PdfDocument) -> Result<Vec<u8>, RenderError> {
    let kfont = Font::new(font.data.clone().into(), font.index).ok_or("font not usable")?;
    let pen = Pen { font, kfont };
    let title = if doc.credit_note {
        "適格返還請求書 / CREDIT NOTE"
    } else {
        "適格請求書 / INVOICE"
    };

    // Lay the table rows out first, so pages can be planned.
    let mut rows: Vec<(usize, Vec<String>)> = Vec::new();
    for (i, l) in doc.lines.iter().enumerate() {
        rows.push((i, pen.wrap(&l.description, 9.0, DESC_W)));
    }

    let mut document = Document::new();
    document.set_metadata(
        Metadata::new()
            .title(format!("{title} {}", doc.number))
            .creator("Chuo-Sogo Procure".into())
            .language("ja".into()),
    );

    // Text blocks under the totals, wrapped once; their height is reserved in full.
    let mut blocks: Vec<(&str, Vec<String>)> = Vec::new();
    for (label, body) in [
        ("返還理由 Reason", doc.reason.as_str()),
        ("備考 Notes", doc.notes.as_str()),
        (
            "お振込先 Payment by bank transfer",
            if doc.credit_note {
                ""
            } else {
                doc.issuer.bank_details.as_str()
            },
        ),
    ] {
        if !body.trim().is_empty() {
            blocks.push((label, pen.wrap(body, 8.5, PAGE_W - 2.0 * MARGIN)));
        }
    }
    let blocks_h: f32 = blocks
        .iter()
        .map(|(_, l)| 16.0 + l.len() as f32 * 11.0)
        .sum();
    let summary_h = 60.0 + doc.per_rate.len() as f32 * 13.0 + 70.0 + blocks_h;
    // A continuation page starts its table at about 120 pt; the summary must fit after it.
    if 120.0 + summary_h > PAGE_H - MARGIN - 40.0 {
        return Err(RenderError::TextTooLong);
    }

    let mut row_iter = rows.into_iter().peekable();
    let mut page_no = 0;
    loop {
        page_no += 1;
        let settings = PageSettings::from_wh(PAGE_W, PAGE_H).ok_or("page size")?;
        let mut page = document.start_page_with(settings);
        let mut s = page.surface();
        s.set_fill(Some(Fill::default()));
        let mut y = MARGIN + 18.0;

        pen.text(&mut s, MARGIN, y, 18.0, title);
        pen.right(
            &mut s,
            PAGE_W - MARGIN,
            y,
            9.0,
            &format!("No. {}", doc.number),
        );
        y += 14.0;
        pen.right(
            &mut s,
            PAGE_W - MARGIN,
            y,
            9.0,
            &format!("発行日 Issue date: {}", doc.issue_date),
        );
        if let Some(due) = &doc.due_date {
            y += 12.0;
            pen.right(
                &mut s,
                PAGE_W - MARGIN,
                y,
                9.0,
                &format!("お支払期限 Due date: {due}"),
            );
        }
        if let Some((n, d)) = &doc.original {
            y += 12.0;
            pen.right(
                &mut s,
                PAGE_W - MARGIN,
                y,
                9.0,
                &format!("対象請求書 Original invoice: {n} ({d})"),
            );
        }

        if page_no == 1 {
            // Recipient (left) and issuer (right).
            y += 22.0;
            let top = y;
            pen.text(
                &mut s,
                MARGIN,
                y,
                13.0,
                &format!("{} 御中", doc.recipient.name),
            );
            rule(&mut s, MARGIN, y + 4.0, 300.0, 0.8);
            y += 18.0;
            for l in pen.wrap(&doc.recipient.address, 9.0, 250.0) {
                pen.text(&mut s, MARGIN, y, 9.0, &l);
                y += 12.0;
            }
            let mut iy = top;
            let ix = 330.0;
            pen.text(&mut s, ix, iy, 11.0, &doc.issuer.legal_name);
            iy += 14.0;
            for l in pen.wrap(&doc.issuer.address, 8.5, PAGE_W - MARGIN - ix) {
                pen.text(&mut s, ix, iy, 8.5, &l);
                iy += 11.0;
            }
            for extra in [&doc.issuer.phone, &doc.issuer.email] {
                if !extra.is_empty() {
                    pen.text(&mut s, ix, iy, 8.5, extra);
                    iy += 11.0;
                }
            }
            pen.text(
                &mut s,
                ix,
                iy,
                9.0,
                &format!(
                    "登録番号 Registration No.: {}",
                    doc.issuer.registration_number
                ),
            );
            iy += 12.0;
            y = y.max(iy) + 14.0;

            // Amount box.
            shade(&mut s, MARGIN, y - 13.0, 300.0, 22.0);
            let label = if doc.credit_note {
                "返還金額（税込） Amount credited"
            } else {
                "ご請求金額（税込） Amount due"
            };
            pen.text(&mut s, MARGIN + 6.0, y + 2.0, 10.0, label);
            pen.right(&mut s, MARGIN + 294.0, y + 2.0, 13.0, &money(doc.total));
            y += 30.0;
        } else {
            y += 24.0;
        }

        // Table header.
        shade(&mut s, MARGIN, y - 11.0, PAGE_W - 2.0 * MARGIN, 16.0);
        pen.text(&mut s, C_NO, y, 8.5, "No.");
        pen.text(&mut s, C_DESC, y, 8.5, "品目 Description");
        pen.right(&mut s, C_QTY_R, y, 8.5, "数量 Qty");
        pen.right(
            &mut s,
            C_UNIT_R,
            y,
            8.5,
            if doc.tax_inclusive {
                "単価(税込) Unit incl."
            } else {
                "単価 Unit price"
            },
        );
        pen.right(&mut s, C_RATE_R, y, 8.5, "税率 Tax");
        pen.right(
            &mut s,
            C_AMT_R,
            y,
            8.5,
            if doc.tax_inclusive {
                "金額(税込) Amount incl."
            } else {
                "金額 Amount"
            },
        );
        y += ROW_H;

        // Rows until the page is full (leave room for the footer).
        let limit = PAGE_H - MARGIN - 40.0;
        while let Some((i, desc)) = row_iter.peek() {
            let h = ROW_H.max(desc.len() as f32 * 11.0 + 4.0);
            if y + h > limit {
                break;
            }
            let l = &doc.lines[*i];
            pen.text(&mut s, C_NO, y, 9.0, &(i + 1).to_string());
            let mut dy = y;
            for part in desc {
                pen.text(&mut s, C_DESC, dy, 9.0, part);
                dy += 11.0;
            }
            let qty = if l.unit.is_empty() {
                l.quantity.to_string()
            } else {
                format!("{} {}", l.quantity, l.unit)
            };
            pen.right(&mut s, C_QTY_R, y, 9.0, &qty);
            pen.right(&mut s, C_UNIT_R, y, 9.0, &money(l.unit_price));
            pen.right(&mut s, C_RATE_R, y, 9.0, &rate_label(l.tax_rate_bps));
            pen.right(&mut s, C_AMT_R, y, 9.0, &money(l.amount));
            y += h;
            rule(&mut s, MARGIN, y - 10.0, PAGE_W - MARGIN, 0.2);
            row_iter.next();
        }

        // Summary on the last page (if it fits; otherwise a continuation page).
        let done = row_iter.peek().is_none() && y + summary_h <= limit;
        if done {
            y += 8.0;
            let lx = 285.0;
            for r in &doc.per_rate {
                let label = match r.rate_bps {
                    0 => "非課税対象 Exempt".to_string(),
                    800 => "8%対象（軽減税率） Reduced 8%".to_string(),
                    b => format!("{}%対象 Standard {}%", b / 100, b / 100),
                };
                if doc.tax_inclusive {
                    let incl = Money::from_minor(r.base.minor() + r.tax.minor(), r.base.currency());
                    pen.text(&mut s, lx, y, 8.5, &format!("{label}（税込）"));
                    pen.right(&mut s, C_RATE_R + 8.0, y, 8.5, &money(incl));
                    pen.right(&mut s, C_AMT_R, y, 8.5, &format!("内税 {}", money(r.tax)));
                } else {
                    pen.text(&mut s, lx, y, 8.5, &label);
                    pen.right(&mut s, C_RATE_R + 8.0, y, 8.5, &money(r.base));
                    pen.right(&mut s, C_AMT_R, y, 8.5, &format!("税 {}", money(r.tax)));
                }
                y += 13.0;
            }
            rule(&mut s, lx, y - 6.0, PAGE_W - MARGIN, 0.5);
            y += 6.0;
            let rows = if doc.tax_inclusive {
                [
                    ("税抜金額 Amount excl. tax", doc.subtotal, 9.0),
                    ("うち消費税 Tax included", doc.tax, 9.0),
                    ("合計（税込） Total", doc.total, 11.0),
                ]
            } else {
                [
                    ("小計 Subtotal", doc.subtotal, 9.0),
                    ("消費税 Consumption tax", doc.tax, 9.0),
                    ("合計 Total", doc.total, 11.0),
                ]
            };
            for (label, amount, size) in rows {
                pen.text(&mut s, lx, y, size, label);
                pen.right(&mut s, C_AMT_R, y, size, &money(amount));
                y += 15.0;
            }
            y += 6.0;
            if doc.lines.iter().any(|l| l.tax_rate_bps == 800) {
                pen.text(
                    &mut s,
                    MARGIN,
                    y,
                    8.0,
                    "※ 軽減税率（8%）対象品目 / reduced tax rate item",
                );
                y += 12.0;
            }
            for (label, lines) in &blocks {
                y += 4.0;
                pen.text(&mut s, MARGIN, y, 8.5, label);
                y += 12.0;
                for l in lines {
                    pen.text(&mut s, MARGIN, y, 8.5, l);
                    y += 11.0;
                }
            }
        }
        pen.right(
            &mut s,
            PAGE_W - MARGIN,
            PAGE_H - MARGIN + 10.0,
            8.0,
            &format!("{} — {}", doc.number, page_no),
        );
        s.finish();
        page.finish();
        if done {
            break;
        }
        if page_no > 60 {
            return Err("document too long".into());
        }
    }
    document
        .finish()
        .map_err(|e| RenderError::Other(format!("pdf: {e:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_formats() {
        assert_eq!(
            money(Money::from_minor(1_234_567, Currency::Jpy)),
            "¥1,234,567"
        );
        assert_eq!(money(Money::from_minor(0, Currency::Jpy)), "¥0");
        assert_eq!(
            money(Money::from_minor(123_456, Currency::Usd)),
            "USD 1,234.56"
        );
        assert_eq!(money(Money::from_minor(5, Currency::Usd)), "USD 0.05");
    }
}

#[cfg(test)]
pub(crate) mod sample {
    use super::*;

    /// A realistic document for tests (mixed rates, a long description, `lines` rows).
    pub fn document(lines: usize) -> PdfDocument {
        let yen = |m| Money::from_minor(m, Currency::Jpy);
        PdfDocument {
            credit_note: false,
            tax_inclusive: false,
            number: "CS-2026-00001".into(),
            issue_date: "2026-11-01".into(),
            due_date: Some("2026-12-01".into()),
            original: None,
            recipient: Party {
                name: "株式会社エー製作所".into(),
                address: "〒100-0001 東京都千代田区千代田1-1".into(),
            },
            issuer: Issuer {
                legal_name: "株式会社中央総合 Chuo-Sogo Inc.".into(),
                registration_number: "T7000012050002".into(),
                address: "〒530-0001 大阪府大阪市北区梅田1-1-1".into(),
                phone: "06-0000-0000".into(),
                email: "billing@chuo-sogo.example".into(),
                bank_details: "三井住友銀行 梅田支店 普通 1234567 カ）チュウオウソウゴウ".into(),
            },
            lines: (0..lines)
                .map(|i| PdfLine {
                    description: if i == 0 {
                        "調達代行サービス料 2026年10月分 — managed procurement service fee for October, including supplier search, RFQ handling and quotation comparison".into()
                    } else {
                        format!("ベアリング 6204-2RS 交換部品 #{i}")
                    },
                    quantity: 2,
                    unit: "個".into(),
                    unit_price: yen(1_250),
                    tax_rate_bps: if i % 3 == 2 { 800 } else { 1000 },
                    amount: yen(2_500),
                })
                .collect(),
            per_rate: vec![
                PdfRate { rate_bps: 1000, base: yen(5_000), tax: yen(500) },
                PdfRate { rate_bps: 800, base: yen(2_500), tax: yen(200) },
            ],
            subtotal: yen(7_500),
            tax: yen(700),
            total: yen(8_200),
            notes: "毎度ありがとうございます。".into(),
            reason: String::new(),
        }
    }

    #[test]
    fn renders_one_and_many_pages() {
        let font =
            PdfFont::load(DEFAULT_FONT_PATH, 0).expect("CJK font installed (fonts-noto-cjk)");
        for n in [3, 120] {
            let pdf = render(&font, &document(n)).unwrap();
            assert!(pdf.len() > 1000);
            assert!(pdf.starts_with(b"%PDF-"), "{n}");
            if let Ok(out) = std::env::var("PDF_SAMPLE_DIR") {
                std::fs::write(format!("{out}/sample-{n}.pdf"), &pdf).unwrap();
            }
        }
        // Text that cannot be printed in full is refused, never cut off.
        let mut long = document(3);
        long.notes = "長い備考。".repeat(2000);
        assert!(matches!(
            render(&font, &long),
            Err(RenderError::TextTooLong)
        ));
        // Deterministic: the stored hash identifies the document.
        let a = render(&font, &document(3)).unwrap();
        let b = render(&font, &document(3)).unwrap();
        assert_eq!(a, b);
    }
}
