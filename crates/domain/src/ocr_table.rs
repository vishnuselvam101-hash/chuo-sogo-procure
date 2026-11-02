//! Rebuilding a table from positioned words: the TSV that `tesseract` (OCR of a scanned page
//! or FAX) and `pdftotext -tsv` (a PDF with a text layer) both write. Words are grouped into
//! lines by height, lines into cells by the gaps between words, and cells into the columns
//! of the header row, so a paper purchase record reaches the same preview as a spreadsheet.
//! Each cell keeps the lowest recognition confidence of its words: numbers the OCR was unsure
//! of are refused, never guessed.

use crate::history;

/// Below this confidence (0–100) a date, quantity, price or part number is not trusted.
pub const MIN_CONFIDENCE: u8 = 70;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub page: u32,
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    /// 0–100; 100 for a PDF text layer.
    pub conf: u8,
    pub text: String,
}

impl Word {
    fn right(&self) -> i32 {
        self.left + self.width
    }
    fn middle_y(&self) -> i32 {
        self.top + self.height / 2
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cell {
    pub text: String,
    pub conf: u8,
    left: i32,
    right: i32,
}

impl Cell {
    pub fn unclear(&self) -> bool {
        !self.text.is_empty() && self.conf < MIN_CONFIDENCE
    }
    fn middle_x(&self) -> i32 {
        self.left + (self.right - self.left) / 2
    }
}

fn number(s: &str) -> Option<i32> {
    // Coordinates are whole pixels (tesseract) or points with decimals (pdftotext).
    let s = s.trim();
    let whole = s.split('.').next()?;
    whole.parse().ok()
}

/// Word rows (level 5) of a tesseract / pdftotext TSV: `level page_num block_num par_num
/// line_num word_num left top width height conf text`. Other levels, empty words and
/// malformed lines are skipped.
pub fn parse_tsv(tsv: &str) -> Vec<Word> {
    tsv.lines()
        .skip(1)
        .filter_map(|line| {
            let f: Vec<&str> = line.splitn(12, '\t').collect();
            if f.len() < 12 || f[0] != "5" {
                return None;
            }
            let text = f[11].trim();
            if text.is_empty() || text.starts_with("###") {
                return None;
            }
            let conf = f[10].trim().split('.').next()?.parse::<i32>().ok()?;
            Some(Word {
                page: f[1].trim().parse().ok()?,
                left: number(f[6])?,
                top: number(f[7])?,
                width: number(f[8])?.max(1),
                height: number(f[9])?.max(1),
                // pdftotext reports -1 (no recognition involved): the text is exact.
                conf: if conf < 0 { 100 } else { conf.min(100) as u8 },
                text: text.to_string(),
            })
        })
        .collect()
}

fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3000}'..='\u{30FF}' | '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}'
        | '\u{F900}'..='\u{FAFF}' | '\u{FF00}'..='\u{FFEF}')
}

fn join(cell: &mut Cell, w: &Word) {
    if cell.text.is_empty() {
        cell.left = w.left;
        cell.conf = w.conf;
    } else {
        // OCR splits Japanese into single characters; only Latin words get a space.
        let glue = !(cell.text.chars().last().is_some_and(is_cjk)
            && w.text.chars().next().is_some_and(is_cjk));
        if glue {
            cell.text.push(' ');
        }
        cell.conf = cell.conf.min(w.conf);
    }
    cell.text.push_str(&w.text);
    cell.right = cell.right.max(w.right());
}

fn median(mut v: Vec<i32>) -> i32 {
    v.sort_unstable();
    v.get(v.len() / 2).copied().unwrap_or(1).max(1)
}

/// Lines of one page, top to bottom, each split into cells at gaps wider than about one
/// character height.
fn page_lines(words: &[&Word]) -> Vec<Vec<Cell>> {
    let mut sorted: Vec<&Word> = words.to_vec();
    sorted.sort_by_key(|w| (w.middle_y(), w.left));
    let mut lines: Vec<Vec<&Word>> = Vec::new();
    for w in sorted {
        let joins = lines.last().is_some_and(|line| {
            let h = median(line.iter().map(|x| x.height).collect()).max(w.height);
            let centre = line.iter().map(|x| x.middle_y()).sum::<i32>() / line.len() as i32;
            (w.middle_y() - centre).abs() * 2 <= h
        });
        match lines.last_mut() {
            Some(line) if joins => line.push(w),
            _ => lines.push(vec![w]),
        }
    }
    lines
        .into_iter()
        .map(|mut line| {
            line.sort_by_key(|w| w.left);
            let gap = median(line.iter().map(|w| w.height).collect()) * 6 / 5;
            let mut cells: Vec<Cell> = Vec::new();
            for w in line {
                let split = cells.last().is_none_or(|c| w.left - c.right > gap);
                if split {
                    cells.push(Cell::default());
                }
                if let Some(c) = cells.last_mut() {
                    join(c, w);
                }
            }
            cells
        })
        .collect()
}

fn texts(lines: &[Vec<Cell>]) -> Vec<Vec<String>> {
    lines
        .iter()
        .map(|l| l.iter().map(|c| c.text.clone()).collect())
        .collect()
}

/// Puts each cell under the header column its centre falls in (between the midpoints of
/// the gaps around that header cell); cells landing in one column are joined.
fn align(line: &[Cell], header: &[Cell]) -> Vec<Cell> {
    let bounds: Vec<i32> = header
        .windows(2)
        .map(|w| w[0].right + (w[1].left - w[0].right) / 2)
        .collect();
    let mut out = vec![Cell::default(); header.len()];
    for c in line {
        let col = bounds.iter().take_while(|b| c.middle_x() > **b).count();
        let slot = &mut out[col];
        if slot.text.is_empty() {
            *slot = c.clone();
        } else {
            slot.text.push(' ');
            slot.text.push_str(&c.text);
            slot.conf = slot.conf.min(c.conf);
            slot.right = c.right;
        }
    }
    out
}

/// The table on the pages, as rows of cells. The first page is returned whole (title lines
/// included; the importer finds the header itself). A later page with the header repeated
/// loses everything down to and including it; a later page without one continues under the
/// last header seen. Without any header the lines are returned unaligned.
pub fn rows_from_words(words: &[Word]) -> Vec<Vec<Cell>> {
    let mut pages: Vec<u32> = words.iter().map(|w| w.page).collect();
    pages.sort_unstable();
    pages.dedup();
    let mut out = Vec::new();
    let mut header: Option<Vec<Cell>> = None;
    for (n, page) in pages.iter().enumerate() {
        let on_page: Vec<&Word> = words.iter().filter(|w| w.page == *page).collect();
        let lines = page_lines(&on_page);
        let start = match history::find_header(&texts(&lines)) {
            Some(h) => {
                header = Some(lines[h].clone());
                if n == 0 { 0 } else { h + 1 }
            }
            None => 0,
        };
        for line in &lines[start..] {
            out.push(match &header {
                Some(head) => align(line, head),
                None => line.clone(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(page: u32, left: i32, top: i32, width: i32, text: &str, conf: u8) -> Word {
        Word {
            page,
            left,
            top,
            width,
            height: 30,
            conf,
            text: text.into(),
        }
    }

    fn text(rows: &[Vec<Cell>]) -> Vec<Vec<String>> {
        texts(rows)
    }

    #[test]
    fn tesseract_and_pdftotext_tsv_are_read() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
                   1\t1\t0\t0\t0\t0\t0\t0\t2480\t3508\t-1\t\n\
                   5\t1\t1\t1\t1\t1\t100\t200\t60\t30\t96.5\t日付\n\
                   5\t1\t1\t1\t1\t2\t100.5\t260.25\t60.0\t30.0\t-1.000000\t2024/1/5\n\
                   5\t1\t1\t1\t1\t3\t1\t1\t1\t1\t90\t###LINE###\n\
                   5\tx\t1\t1\t1\t3\t1\t1\t1\t1\t90\tbroken\n";
        let words = parse_tsv(tsv);
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].conf, 96);
        assert_eq!(
            (words[1].left, words[1].top, words[1].conf),
            (100, 260, 100)
        );
    }

    #[test]
    fn japanese_characters_are_joined_and_columns_split_by_gaps() {
        // 日付 | 品名 | 単価 ; one data row whose description is split per character by OCR.
        let words = vec![
            w(1, 100, 100, 60, "日付", 95),
            w(1, 400, 100, 60, "品名", 95),
            w(1, 800, 100, 60, "数量", 95),
            w(1, 1000, 100, 60, "単価", 95),
            w(1, 100, 160, 150, "2024/1/5", 93),
            w(1, 400, 162, 30, "六", 91),
            w(1, 432, 158, 30, "角", 88),
            w(1, 464, 160, 30, "ボ", 90),
            w(1, 496, 160, 60, "ルト", 90),
            w(1, 566, 160, 40, "M8", 92),
            w(1, 820, 160, 40, "100", 97),
            w(1, 1000, 160, 60, "12.5", 64),
        ];
        let rows = rows_from_words(&words);
        assert_eq!(
            text(&rows),
            vec![
                vec!["日付", "品名", "数量", "単価"],
                vec!["2024/1/5", "六角ボルト M8", "100", "12.5"],
            ]
        );
        assert!(rows[1][3].unclear());
        assert!(!rows[1][1].unclear());
        assert_eq!(rows[1][1].conf, 88);
    }

    #[test]
    fn right_aligned_numbers_and_empty_cells_land_in_their_columns() {
        let words = vec![
            w(1, 100, 100, 60, "日付", 95),
            w(1, 300, 100, 90, "仕入先", 95),
            w(1, 600, 100, 60, "品名", 95),
            w(1, 900, 100, 60, "単価", 95),
            w(1, 1100, 100, 60, "金額", 95),
            // No supplier on this row; the price sits right-aligned under 単価.
            w(1, 100, 150, 150, "2024/2/1", 95),
            w(1, 600, 150, 100, "ナット", 95),
            w(1, 930, 150, 50, "8", 95),
            w(1, 1110, 150, 70, "800", 95),
        ];
        let rows = text(&rows_from_words(&words));
        assert_eq!(rows[1], vec!["2024/2/1", "", "ナット", "8", "800"]);
    }

    #[test]
    fn later_pages_drop_their_repeated_header_and_titles() {
        let header = |page| {
            vec![
                w(page, 100, 100, 60, "日付", 95),
                w(page, 400, 100, 60, "品名", 95),
                w(page, 800, 100, 60, "数量", 95),
                w(page, 1000, 100, 60, "単価", 95),
            ]
        };
        let mut words = vec![w(1, 400, 20, 200, "購買記録", 95)];
        words.extend(header(1));
        words.push(w(1, 100, 160, 150, "2024/1/5", 95));
        words.push(w(1, 400, 160, 60, "ボルト", 95));
        words.push(w(1, 1000, 160, 60, "10", 95));
        words.push(w(2, 400, 20, 200, "購買記録 2", 95));
        words.extend(header(2));
        words.push(w(2, 100, 160, 150, "2024/1/6", 95));
        words.push(w(2, 400, 160, 60, "ナット", 95));
        words.push(w(2, 1000, 160, 60, "3", 95));
        // Page 3 has no header: it continues under page 2's columns.
        words.push(w(3, 100, 100, 150, "2024/1/7", 95));
        words.push(w(3, 400, 100, 60, "座金", 95));
        words.push(w(3, 1000, 100, 60, "1", 95));
        let rows = text(&rows_from_words(&words));
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0], vec!["", "購買記録", "", ""]);
        assert_eq!(rows[1][0], "日付");
        assert_eq!(rows[3], vec!["2024/1/6", "ナット", "", "3"]);
        assert_eq!(rows[4], vec!["2024/1/7", "座金", "", "1"]);
    }

    #[test]
    fn without_a_header_lines_come_back_as_they_are() {
        let words = vec![
            w(1, 100, 100, 60, "hello", 95),
            w(1, 600, 100, 60, "world", 95),
        ];
        assert_eq!(text(&rows_from_words(&words)), vec![vec!["hello", "world"]]);
        assert!(rows_from_words(&[]).is_empty());
    }
}
