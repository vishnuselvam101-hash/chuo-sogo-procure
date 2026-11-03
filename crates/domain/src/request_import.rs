//! Purchase requests from a factory's CSV / Excel file (CEO specification 2026-11-03: at first
//! we exchange CSV files with the factory instead of connecting to its ERP). One row is one
//! line; rows with the same request name become one request. Japanese or English headers.
//! Deterministic: a row that cannot be read exactly is reported with a reason, never guessed.

use crate::history::{Day, half_width, header_key, is_blank, parse_date};

/// Lines on one request (`procure_server::api::requests::MAX_ITEMS`).
pub const MAX_LINES_PER_REQUEST: usize = 200;
/// Requests one file may create.
pub const MAX_REQUESTS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Request,
    Description,
    Manufacturer,
    Mpn,
    Quantity,
    Unit,
    NeededBy,
    DeliveryLocation,
    Notes,
}

impl Column {
    const ALL: [Column; 9] = [
        Column::Request,
        Column::Description,
        Column::Manufacturer,
        Column::Mpn,
        Column::Quantity,
        Column::Unit,
        Column::NeededBy,
        Column::DeliveryLocation,
        Column::Notes,
    ];

    fn synonyms(self) -> &'static [&'static str] {
        match self {
            Column::Request => &[
                "request",
                "requesttitle",
                "title",
                "件名",
                "依頼名",
                "依頼件名",
                "購買依頼",
                "依頼番号",
                "requestno",
                "requestnumber",
            ],
            Column::Description => &[
                "description",
                "item",
                "itemname",
                "product",
                "品名",
                "品目",
                "名称",
                "商品名",
                "品名規格",
            ],
            Column::Manufacturer => &["manufacturer", "maker", "brand", "メーカー", "メーカー名"],
            Column::Mpn => &[
                "mpn",
                "partnumber",
                "partno",
                "model",
                "modelnumber",
                "型番",
                "品番",
                "型式",
                "部品番号",
            ],
            Column::Quantity => &["quantity", "qty", "数量", "必要数", "発注数"],
            Column::Unit => &["unit", "uom", "単位"],
            Column::NeededBy => &[
                "neededby",
                "needed",
                "duedate",
                "deliverydate",
                "納期",
                "希望納期",
                "必要日",
            ],
            Column::DeliveryLocation => &[
                "deliverylocation",
                "deliverto",
                "location",
                "納品先",
                "納入場所",
                "納入先",
            ],
            Column::Notes => &["notes", "remarks", "memo", "備考", "メモ", "仕様"],
        }
    }
}

/// Which column each header cell names (`None` = ignored). A column is taken by its first cell.
pub fn detect_columns(header: &[String]) -> Vec<Option<Column>> {
    let mut taken: Vec<Column> = Vec::new();
    header
        .iter()
        .map(|h| {
            let key = header_key(h);
            let col = Column::ALL
                .into_iter()
                .find(|c| c.synonyms().contains(&key.as_str()))?;
            if taken.contains(&col) {
                return None;
            }
            taken.push(col);
            Some(col)
        })
        .collect()
}

/// The first of the leading rows naming the item (description or model number) and the
/// quantity.
pub fn find_header(rows: &[Vec<String>]) -> Option<usize> {
    rows.iter().take(15).position(|row| {
        let cols = detect_columns(row);
        cols.contains(&Some(Column::Quantity))
            && cols
                .iter()
                .any(|c| matches!(c, Some(Column::Description | Column::Mpn)))
    })
}

/// Unit codes the platform uses (lowercase letters and `_`). Common Japanese counters map to
/// them; anything else is refused rather than guessed.
pub fn unit_code(raw: &str) -> Option<String> {
    let s = half_width(raw.trim()).to_lowercase();
    let mapped = match s.as_str() {
        "" | "個" | "ヶ" | "コ" | "本" | "ea" | "each" | "pc" | "pcs" | "piece" | "pieces" => {
            "pcs"
        }
        "箱" | "ケース" | "box" | "boxes" | "case" => "box",
        "セット" | "組" | "式" | "set" | "sets" => "set",
        "枚" | "sheet" | "sheets" => "sheet",
        "台" | "unit" | "units" => "unit",
        "巻" | "roll" | "rolls" => "roll",
        "袋" | "bag" | "bags" => "bag",
        "缶" | "can" | "cans" => "can",
        "パック" | "pack" | "packs" => "pack",
        "対" | "pair" | "pairs" | "足" => "pair",
        "m" | "メートル" => "m",
        "kg" | "キロ" => "kg",
        "l" | "リットル" => "l",
        other
            if !other.is_empty()
                && other.len() <= 16
                && other.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') =>
        {
            other
        }
        _ => return None,
    };
    Some(mapped.to_string())
}

/// Why a row was not imported. Codes are shown to the person importing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineError {
    NoItem,
    Quantity,
    Unit,
    NeededBy,
    TooLong,
    TooManyLines,
    TooManyRequests,
}

impl LineError {
    pub const fn code(self) -> &'static str {
        match self {
            LineError::NoItem => "no_item",
            LineError::Quantity => "quantity",
            LineError::Unit => "unit",
            LineError::NeededBy => "needed_by",
            LineError::TooLong => "too_long",
            LineError::TooManyLines => "too_many_lines",
            LineError::TooManyRequests => "too_many_requests",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub description: String,
    pub manufacturer: String,
    pub mpn: String,
    pub quantity: u32,
    pub unit: String,
    pub notes: String,
}

/// One request to create: its name (the request column, or empty when the file has none),
/// the earliest needed-by date and the first delivery location given, and its lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    pub needed_by: Option<Day>,
    pub delivery_location: String,
    pub lines: Vec<Line>,
    /// 1-based file rows of the lines, for reporting.
    pub rows: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Parsed {
    pub groups: Vec<Group>,
    /// (1-based file row, reason).
    pub skipped: Vec<(usize, LineError)>,
}

fn cell(row: &[String], cols: &[Option<Column>], want: Column) -> String {
    cols.iter()
        .position(|c| *c == Some(want))
        .and_then(|i| row.get(i))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn quantity(raw: &str) -> Option<u32> {
    let s: String = half_width(raw).chars().filter(|c| *c != ',').collect();
    let s = s.trim();
    // "10.0" from a spreadsheet is ten; "10.5" is not a whole quantity.
    let whole = match s.split_once('.') {
        Some((w, f)) if f.chars().all(|c| c == '0') => w,
        Some(_) => return None,
        None => s,
    };
    let q: u32 = whole.parse().ok()?;
    (1..=1_000_000_000).contains(&q).then_some(q)
}

fn fits(s: &str, max: usize) -> bool {
    s.chars().count() <= max
}

/// Reads the rows below `header`.
pub fn parse(rows: &[Vec<String>], header: usize) -> Parsed {
    let cols = detect_columns(&rows[header]);
    let mut out = Parsed::default();
    for (i, row) in rows.iter().enumerate().skip(header + 1) {
        let file_row = i + 1;
        if is_blank(row) {
            continue;
        }
        let get = |c| cell(row, &cols, c);
        let (description, mpn) = (get(Column::Description), get(Column::Mpn));
        let line = (|| {
            if description.is_empty() && mpn.is_empty() {
                return Err(LineError::NoItem);
            }
            let quantity = quantity(&get(Column::Quantity)).ok_or(LineError::Quantity)?;
            let unit = unit_code(&get(Column::Unit)).ok_or(LineError::Unit)?;
            let raw_date = get(Column::NeededBy);
            let needed_by = if raw_date.is_empty() {
                None
            } else {
                Some(parse_date(&raw_date).ok_or(LineError::NeededBy)?)
            };
            let line = Line {
                // A line with only a model number is still described by it.
                description: if description.is_empty() {
                    mpn.clone()
                } else {
                    description.clone()
                },
                manufacturer: get(Column::Manufacturer),
                mpn: mpn.clone(),
                quantity,
                unit,
                notes: get(Column::Notes),
            };
            let location = get(Column::DeliveryLocation);
            let name = get(Column::Request);
            if !fits(&line.description, 1000)
                || !fits(&line.manufacturer, 200)
                || !fits(&line.mpn, 100)
                || !fits(&line.notes, 2000)
                || !fits(&location, 500)
                || !fits(&name, 200)
            {
                return Err(LineError::TooLong);
            }
            Ok((name, needed_by, location, line))
        })();
        let (name, needed_by, location, line) = match line {
            Ok(v) => v,
            Err(e) => {
                out.skipped.push((file_row, e));
                continue;
            }
        };
        let at = match out.groups.iter().position(|g| g.name == name) {
            Some(at) => at,
            None if out.groups.len() >= MAX_REQUESTS => {
                out.skipped.push((file_row, LineError::TooManyRequests));
                continue;
            }
            None => {
                out.groups.push(Group {
                    name: name.clone(),
                    needed_by: None,
                    delivery_location: String::new(),
                    lines: Vec::new(),
                    rows: Vec::new(),
                });
                out.groups.len() - 1
            }
        };
        let g = &mut out.groups[at];
        if g.lines.len() >= MAX_LINES_PER_REQUEST {
            out.skipped.push((file_row, LineError::TooManyLines));
            continue;
        }
        if let Some(d) = needed_by
            && g.needed_by
                .is_none_or(|cur| d.epoch_days() < cur.epoch_days())
        {
            g.needed_by = Some(d);
        }
        if g.delivery_location.is_empty() {
            g.delivery_location = location;
        }
        g.lines.push(line);
        g.rows.push(file_row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(t: &[&[&str]]) -> Vec<Vec<String>> {
        t.iter()
            .map(|r| r.iter().map(|c| c.to_string()).collect())
            .collect()
    }

    #[test]
    fn japanese_headers_group_rows_into_requests() {
        let r = rows(&[
            &["購買依頼一覧", "", "", "", ""],
            &["件名", "品名", "型番", "数量", "単位", "希望納期", "納品先"],
            &[
                "第1工場 消耗品",
                "ボールベアリング",
                "6204ZZ",
                "100",
                "個",
                "2026/12/10",
                "第1工場 資材倉庫",
            ],
            &[
                "第1工場 消耗品",
                "",
                "HSR15",
                "４",
                "セット",
                "2026/12/01",
                "",
            ],
            &["電気材料", "VVFケーブル 2.0mm", "", "3", "巻", "", ""],
            &["", "", "", "", "", "", ""],
            &["電気材料", "端子台", "TB-15", "2.5", "個", "", ""],
            &["電気材料", "ブレーカー", "BKW-50", "1", "ダース", "", ""],
            &["電気材料", "", "", "1", "個", "", ""],
        ]);
        let h = find_header(&r).unwrap();
        assert_eq!(h, 1);
        let p = parse(&r, h);
        assert_eq!(p.groups.len(), 2);
        let g = &p.groups[0];
        assert_eq!(g.name, "第1工場 消耗品");
        assert_eq!(g.lines.len(), 2);
        assert_eq!(g.lines[0].unit, "pcs");
        assert_eq!(g.lines[1].description, "HSR15");
        assert_eq!(g.lines[1].quantity, 4);
        assert_eq!(g.lines[1].unit, "set");
        // The earliest date of the lines, the first location given.
        assert_eq!(g.needed_by, Day::new(2026, 12, 1));
        assert_eq!(g.delivery_location, "第1工場 資材倉庫");
        assert_eq!(g.rows, vec![3, 4]);
        assert_eq!(p.groups[1].lines[0].unit, "roll");
        assert_eq!(
            p.skipped,
            vec![
                (7, LineError::Quantity),
                (8, LineError::Unit),
                (9, LineError::NoItem)
            ]
        );
    }

    #[test]
    fn english_file_without_request_column_is_one_request() {
        let r = rows(&[
            &["Description", "Part number", "Qty", "Unit", "Needed by"],
            &["Cutting oil 18L", "", "2", "can", "2026-12-20"],
            &["End mill 6mm", "EM-6", "10", "", "next week"],
            &["Glove", "", "1,000", "pairs", ""],
        ]);
        let p = parse(&r, find_header(&r).unwrap());
        assert_eq!(p.groups.len(), 1);
        assert_eq!(p.groups[0].name, "");
        assert_eq!(p.groups[0].lines.len(), 2);
        assert_eq!(p.groups[0].lines[1].quantity, 1000);
        assert_eq!(p.groups[0].lines[1].unit, "pair");
        assert_eq!(p.skipped, vec![(3, LineError::NeededBy)]);
    }

    #[test]
    fn limits() {
        let mut t = vec![vec!["品名".to_string(), "数量".to_string()]];
        for i in 0..=MAX_LINES_PER_REQUEST {
            t.push(vec![format!("item {i}"), "1".into()]);
        }
        let p = parse(&t, 0);
        assert_eq!(p.groups[0].lines.len(), MAX_LINES_PER_REQUEST);
        assert_eq!(
            p.skipped,
            vec![(MAX_LINES_PER_REQUEST + 2, LineError::TooManyLines)]
        );
        assert!(find_header(&rows(&[&["品名", "単価"]])).is_none());
        assert_eq!(quantity("0"), None);
        assert_eq!(quantity("12.00"), Some(12));
        assert_eq!(unit_code("Box"), Some("box".into()));
        assert_eq!(unit_code("ダース"), None);
    }
}
