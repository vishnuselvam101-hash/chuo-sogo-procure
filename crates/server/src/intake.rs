//! Turning a supplier's voice note or catalogue file into plain text for the AI listing
//! assistant (Phase D). Runs on this server only: audio goes through `ffmpeg` and a
//! whisper.cpp-compatible command (`PROCURE_STT_COMMAND`, `PROCURE_STT_MODEL`), PDFs through
//! `pdftotext`; spreadsheets and CSV are read in-process. Raw audio is never stored: it lives
//! in a private temporary directory for the duration of the call.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use procure_ai::provider::BoxFuture;
use tokio::process::Command;

/// Longest voice note accepted.
pub const MAX_AUDIO_SECONDS: u32 = 300;
/// Text handed to the model (same limit as typed input).
pub const MAX_TEXT_CHARS: usize = 20_000;
const TOOL_TIMEOUT: Duration = Duration::from_secs(240);

#[derive(Debug, PartialEq, Eq)]
pub enum IntakeError {
    /// Not an accepted audio/document type (by content).
    Unsupported,
    /// Audio longer than `MAX_AUDIO_SECONDS`.
    TooLong,
    /// Nothing readable in it (silent audio, scanned PDF without text).
    Empty,
    /// The server has no speech-to-text configured.
    SpeechUnavailable,
    /// A tool failed or timed out.
    Failed,
    /// A table with more rows than one import takes, or a scan with too many pages/pixels.
    TooLarge,
    /// A scanned record, but this server has no OCR installed.
    OcrUnavailable,
}

impl IntakeError {
    pub const fn code(&self) -> &'static str {
        match self {
            IntakeError::Unsupported => "file_type",
            IntakeError::TooLong => "audio_too_long",
            IntakeError::Empty => "nothing_readable",
            IntakeError::SpeechUnavailable => "speech_unavailable",
            IntakeError::Failed => "intake_failed",
            IntakeError::TooLarge => "too_many_rows",
            IntakeError::OcrUnavailable => "ocr_unavailable",
        }
    }
}

/// Audio container by content: wav, mp3, m4a/mp4, ogg/opus, webm, flac.
pub fn audio_kind(b: &[u8]) -> Option<&'static str> {
    if b.len() < 12 {
        return None;
    }
    if &b[0..4] == b"RIFF" && &b[8..12] == b"WAVE" {
        Some("wav")
    } else if b.starts_with(b"ID3") || (b[0] == 0xFF && (b[1] & 0xE0) == 0xE0) {
        Some("mp3")
    } else if &b[4..8] == b"ftyp" {
        Some("m4a")
    } else if b.starts_with(b"OggS") {
        Some("ogg")
    } else if b.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        Some("webm")
    } else if b.starts_with(b"fLaC") {
        Some("flac")
    } else {
        None
    }
}

fn demuxer(kind: &str) -> &'static str {
    match kind {
        "wav" => "wav",
        "mp3" => "mp3",
        "m4a" => "mov",
        "ogg" => "ogg",
        "webm" => "matroska",
        _ => "flac",
    }
}

pub trait Transcriber: Send + Sync {
    fn name(&self) -> &str;
    /// Plain text of the speech in `audio` (`kind` from [`audio_kind`]); `language` is
    /// `ja`, `en`, `ta`, `hi` or `None` for automatic detection.
    fn transcribe<'a>(
        &'a self,
        audio: &'a [u8],
        kind: &'a str,
        language: Option<&'a str>,
    ) -> BoxFuture<'a, Result<String, IntakeError>>;
}

/// A private scratch directory, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    /// Under `PROCURE_WORK_DIR` (default `./var/work`; the service uses its own state
    /// directory, never a shared temp directory). Created owner-only in one step, with a
    /// random name, failing if anything already exists there.
    fn new() -> Result<Self, IntakeError> {
        let base = PathBuf::from(
            std::env::var("PROCURE_WORK_DIR").unwrap_or_else(|_| "./var/work".into()),
        );
        std::fs::create_dir_all(&base).map_err(|_| IntakeError::Failed)?;
        let dir = base.join(format!("intake-{}", uuid::Uuid::new_v4()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&dir).map_err(|_| IntakeError::Failed)?;
        Ok(Scratch(dir))
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Most bytes read from a tool's output (enough for `MAX_TEXT_CHARS` of any script).
const MAX_TOOL_OUTPUT: u64 = (MAX_TEXT_CHARS as u64) * 4 + 4096;

/// Runs a tool with a timeout; at most `MAX_TOOL_OUTPUT` bytes of stdout on success (the
/// tool is killed if it produces more, or runs too long).
async fn run_tool(program: &Path, args: &[&std::ffi::OsStr]) -> Result<Vec<u8>, IntakeError> {
    use tokio::io::AsyncReadExt;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| IntakeError::Failed)?;
    let mut stdout = child.stdout.take().ok_or(IntakeError::Failed)?;
    let work = async {
        let mut out = Vec::new();
        (&mut stdout)
            .take(MAX_TOOL_OUTPUT)
            .read_to_end(&mut out)
            .await
            .map_err(|_| IntakeError::Failed)?;
        // Output beyond the cap: stop reading and let the kill below end the tool.
        let mut probe = [0u8; 1];
        if stdout
            .read(&mut probe)
            .await
            .map_err(|_| IntakeError::Failed)?
            > 0
        {
            return Ok((out, None));
        }
        let status = child.wait().await.map_err(|_| IntakeError::Failed)?;
        Ok::<_, IntakeError>((out, Some(status)))
    };
    match tokio::time::timeout(TOOL_TIMEOUT, work).await {
        Ok(Ok((out, Some(status)))) if status.success() => Ok(out),
        // Truncated output is still usable text (the model input is capped anyway).
        Ok(Ok((out, None))) => Ok(out),
        _ => Err(IntakeError::Failed),
    }
}

/// ffmpeg → 16 kHz mono WAV → whisper.cpp (`-nt -np`, text on stdout).
pub struct CommandTranscriber {
    pub ffmpeg: PathBuf,
    pub whisper: PathBuf,
    pub model: PathBuf,
}

impl CommandTranscriber {
    /// From `PROCURE_STT_COMMAND` and `PROCURE_STT_MODEL` (both required) and
    /// `PROCURE_FFMPEG` (default `ffmpeg`). `None` when speech is not configured.
    pub fn from_env() -> Option<Self> {
        let whisper = std::env::var("PROCURE_STT_COMMAND").ok()?;
        let model = std::env::var("PROCURE_STT_MODEL").ok()?;
        let ffmpeg = std::env::var("PROCURE_FFMPEG").unwrap_or_else(|_| "ffmpeg".into());
        Some(CommandTranscriber {
            ffmpeg: ffmpeg.into(),
            whisper: whisper.into(),
            model: model.into(),
        })
    }
}

impl Transcriber for CommandTranscriber {
    fn name(&self) -> &str {
        "whisper.cpp"
    }

    fn transcribe<'a>(
        &'a self,
        audio: &'a [u8],
        kind: &'a str,
        language: Option<&'a str>,
    ) -> BoxFuture<'a, Result<String, IntakeError>> {
        Box::pin(async move {
            let scratch = Scratch::new()?;
            let input = scratch.path(&format!("input.{kind}"));
            let wav = scratch.path("audio.wav");
            tokio::fs::write(&input, audio)
                .await
                .map_err(|_| IntakeError::Failed)?;
            // One second over the limit, so "too long" can be told apart from "exactly 5 min".
            let limit = (MAX_AUDIO_SECONDS + 1).to_string();
            run_tool(
                &self.ffmpeg,
                &[
                    "-nostdin".as_ref(),
                    "-hide_banner".as_ref(),
                    "-loglevel".as_ref(),
                    "error".as_ref(),
                    // Only the local file, read with the demuxer its content was checked as:
                    // no playlists, concat lists or network protocols.
                    "-protocol_whitelist".as_ref(),
                    "file".as_ref(),
                    "-f".as_ref(),
                    demuxer(kind).as_ref(),
                    "-i".as_ref(),
                    input.as_os_str(),
                    "-t".as_ref(),
                    limit.as_ref(),
                    "-ac".as_ref(),
                    "1".as_ref(),
                    "-ar".as_ref(),
                    "16000".as_ref(),
                    "-c:a".as_ref(),
                    "pcm_s16le".as_ref(),
                    "-f".as_ref(),
                    "wav".as_ref(),
                    wav.as_os_str(),
                ],
            )
            .await?;
            let size = tokio::fs::metadata(&wav)
                .await
                .map_err(|_| IntakeError::Failed)?
                .len();
            // 16 kHz × 2 bytes per sample, plus the header.
            if size > u64::from(MAX_AUDIO_SECONDS) * 32_000 + 1024 {
                return Err(IntakeError::TooLong);
            }
            let lang = language.unwrap_or("auto");
            let out = run_tool(
                &self.whisper,
                &[
                    "-m".as_ref(),
                    self.model.as_os_str(),
                    "-f".as_ref(),
                    wav.as_os_str(),
                    "-l".as_ref(),
                    lang.as_ref(),
                    "-nt".as_ref(),
                    "-np".as_ref(),
                ],
            )
            .await?;
            let text = String::from_utf8_lossy(&out).trim().to_string();
            if text.is_empty() {
                return Err(IntakeError::Empty);
            }
            Ok(clip(&text))
        })
    }
}

fn clip(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .take(MAX_TEXT_CHARS)
        .collect()
}

/// Text of a catalogue or data sheet: PDF (text layer), Excel (.xlsx/.xls), CSV or plain
/// text (UTF-8, or Shift_JIS as exported by Japanese Excel).
pub async fn document_text(file_name: &str, bytes: &[u8]) -> Result<String, IntakeError> {
    let ext = Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let text = if bytes.starts_with(b"%PDF-") {
        pdf_text(bytes).await?
    } else if ext == "xlsx" && bytes.starts_with(b"PK\x03\x04") {
        let owned = bytes.to_vec();
        tokio::task::spawn_blocking(move || {
            check_workbook(&owned)?;
            sheet_text(&owned)
        })
        .await
        .map_err(|_| IntakeError::Failed)??
    } else if matches!(ext.as_str(), "csv" | "txt" | "tsv") && !bytes.contains(&0) {
        decode_text(bytes)
    } else {
        return Err(IntakeError::Unsupported);
    };
    let text = clip(text.trim());
    if text.chars().filter(|c| c.is_alphanumeric()).count() < 3 {
        return Err(IntakeError::Empty);
    }
    Ok(text)
}

async fn pdf_text(bytes: &[u8]) -> Result<String, IntakeError> {
    let tool = std::env::var("PROCURE_PDFTOTEXT").unwrap_or_else(|_| "pdftotext".into());
    let scratch = Scratch::new()?;
    let input = scratch.path("input.pdf");
    tokio::fs::write(&input, bytes)
        .await
        .map_err(|_| IntakeError::Failed)?;
    let out = run_tool(
        Path::new(&tool),
        &[
            "-layout".as_ref(),
            "-l".as_ref(),
            "30".as_ref(),
            "-enc".as_ref(),
            "UTF-8".as_ref(),
            input.as_os_str(),
            "-".as_ref(),
        ],
    )
    .await?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Largest decompressed workbook accepted, and the largest sheet area declared.
const MAX_WORKBOOK_BYTES: u64 = 40 * 1024 * 1024;
const MAX_WORKBOOK_ENTRIES: usize = 2_000;
const MAX_SHEET_CELLS: u64 = 2_000_000;

/// Zip-bomb and huge-sheet check before the spreadsheet parser runs: the real decompressed
/// size of every entry is counted (not the size the file claims), and each sheet's declared
/// dimension must be reasonable.
fn check_workbook(bytes: &[u8]) -> Result<(), IntakeError> {
    use std::io::Read;
    let mut zip =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| IntakeError::Unsupported)?;
    if zip.len() > MAX_WORKBOOK_ENTRIES {
        return Err(IntakeError::Unsupported);
    }
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|_| IntakeError::Unsupported)?;
        let sheet = entry
            .name()
            .map(|n| n.starts_with("xl/worksheets/") && n.ends_with(".xml"))
            .unwrap_or(false);
        let mut head = Vec::new();
        let limit = MAX_WORKBOOK_BYTES - total + 1;
        let read = std::io::copy(
            &mut (&mut entry).take(limit),
            &mut LimitedHead {
                head: &mut head,
                keep: if sheet { 4096 } else { 0 },
            },
        )
        .map_err(|_| IntakeError::Unsupported)?;
        total += read;
        if total > MAX_WORKBOOK_BYTES {
            return Err(IntakeError::Unsupported);
        }
        if sheet && dimension_cells(&String::from_utf8_lossy(&head)) > MAX_SHEET_CELLS {
            return Err(IntakeError::Unsupported);
        }
    }
    Ok(())
}

/// Writer that counts everything but keeps only the first `keep` bytes.
struct LimitedHead<'a> {
    head: &'a mut Vec<u8>,
    keep: usize,
}

impl std::io::Write for LimitedHead<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let room = self.keep.saturating_sub(self.head.len());
        self.head.extend_from_slice(&buf[..room.min(buf.len())]);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Cells in `<dimension ref="A1:XFD1048576"/>` (0 when absent).
fn dimension_cells(xml: &str) -> u64 {
    let Some(start) = xml.find("<dimension ref=\"") else {
        return 0;
    };
    let rest = &xml[start + 16..];
    let Some(end) = rest.find('"') else {
        return 0;
    };
    let cell = |c: &str| -> Option<(u64, u64)> {
        let letters: String = c.chars().take_while(char::is_ascii_alphabetic).collect();
        let row: u64 = c[letters.len()..].parse().ok()?;
        let col = letters.bytes().fold(0u64, |acc, b| {
            acc * 26 + u64::from(b.to_ascii_uppercase() - b'A' + 1)
        });
        Some((col, row))
    };
    match rest[..end].split_once(':') {
        Some((a, b)) => match (cell(a), cell(b)) {
            (Some((c1, r1)), Some((c2, r2))) => {
                (c2.saturating_sub(c1) + 1).saturating_mul(r2.saturating_sub(r1) + 1)
            }
            _ => u64::MAX,
        },
        None => 1,
    }
}

fn sheet_text(bytes: &[u8]) -> Result<String, IntakeError> {
    use calamine::Reader;
    let mut book = calamine::open_workbook_auto_from_rs(std::io::Cursor::new(bytes.to_vec()))
        .map_err(|_| IntakeError::Unsupported)?;
    let mut out = String::new();
    for name in book.sheet_names().to_vec() {
        let Ok(range) = book.worksheet_range(&name) else {
            continue;
        };
        out.push_str(&format!("[{name}]\n"));
        for row in range.rows() {
            let cells: Vec<String> = row.iter().map(ToString::to_string).collect();
            if cells.iter().any(|c| !c.is_empty()) {
                out.push_str(&cells.join("\t"));
                out.push('\n');
            }
            if out.len() > MAX_TEXT_CHARS * 4 {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

/// Most rows a purchase-history file may hold (one import); larger exports are split.
pub const MAX_TABLE_ROWS: usize = 50_000;
/// Columns beyond this are ignored (no purchase export needs more).
pub const MAX_TABLE_COLUMNS: usize = 60;

/// A purchase table as read from a file.
#[derive(Debug, PartialEq, Eq)]
pub struct Table {
    pub rows: Vec<Vec<String>>,
    /// Per cell: recognised with low confidence (OCR only; empty for exact sources).
    pub unclear: Vec<Vec<bool>>,
    /// Read by OCR from a scan, photo or FAX (the numbers must be checked against the paper).
    pub ocr: bool,
}

impl Table {
    fn exact(rows: Vec<Vec<String>>) -> Self {
        Table {
            rows,
            unclear: Vec::new(),
            ocr: false,
        }
    }

    pub fn is_unclear(&self, row: usize, col: usize) -> bool {
        self.unclear
            .get(row)
            .and_then(|r| r.get(col))
            .copied()
            .unwrap_or(false)
    }
}

/// A spreadsheet (.xlsx, first sheet that looks like a purchase table, else the first sheet),
/// a CSV/TSV file (UTF-8 or Shift_JIS), a PDF (its text layer, else OCR of its pages) or a
/// scan / photo / FAX (PNG, JPEG, TIFF; OCR) as rows of cell text. Dates in Excel cells come
/// out as `YYYY-MM-DD`; numbers keep their exact decimal text.
pub async fn table_rows(file_name: &str, bytes: &[u8]) -> Result<Table, IntakeError> {
    let ext = Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let table = if bytes.starts_with(b"%PDF-") {
        pdf_table(bytes).await?
    } else if let Some(kind) = image_kind(bytes) {
        ocr_image(bytes, kind, &tool("PROCURE_TESSERACT", "tesseract")).await?
    } else if ext == "xlsx" && bytes.starts_with(b"PK\x03\x04") {
        let owned = bytes.to_vec();
        Table::exact(
            tokio::task::spawn_blocking(move || {
                check_workbook(&owned)?;
                sheet_rows(&owned)
            })
            .await
            .map_err(|_| IntakeError::Failed)??,
        )
    } else if matches!(ext.as_str(), "csv" | "tsv" | "txt") && !bytes.contains(&0) {
        Table::exact(delimited_rows(&decode_text(bytes))?)
    } else {
        return Err(IntakeError::Unsupported);
    };
    if table
        .rows
        .iter()
        .all(|r| procure_domain::history::is_blank(r))
    {
        return Err(IntakeError::Empty);
    }
    if table.rows.len() > MAX_TABLE_ROWS + 20 {
        return Err(IntakeError::TooLarge);
    }
    Ok(table)
}

/// Pages read from one scanned document, and the largest page accepted (an A3 sheet at
/// 300 dpi is about 17.5 million pixels).
pub const MAX_OCR_PAGES: usize = 10;
const MAX_OCR_PIXELS: u64 = 40_000_000;
/// Largest TSV read back from `pdftotext` / `tesseract`.
const MAX_TSV_BYTES: u64 = 16 * 1024 * 1024;

fn tool(var: &str, default: &str) -> PathBuf {
    std::env::var(var).unwrap_or_else(|_| default.into()).into()
}

/// Whether `program` can be started: an existing path, or a name found on `PATH`.
fn installed(program: &Path) -> bool {
    if program.components().count() > 1 {
        return program.is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
}

async fn read_tsv(path: &Path) -> Result<String, IntakeError> {
    let size = tokio::fs::metadata(path)
        .await
        .map_err(|_| IntakeError::Failed)?
        .len();
    if size > MAX_TSV_BYTES {
        return Err(IntakeError::TooLarge);
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|_| IntakeError::Failed)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn words_table(words: &[procure_domain::ocr_table::Word], ocr: bool) -> Table {
    let cells = procure_domain::ocr_table::rows_from_words(words);
    Table {
        rows: cells
            .iter()
            .map(|r| r.iter().map(|c| c.text.clone()).collect())
            .collect(),
        unclear: if ocr {
            cells
                .iter()
                .map(|r| r.iter().map(|c| c.unclear()).collect())
                .collect()
        } else {
            Vec::new()
        },
        ocr,
    }
}

/// The text layer when the PDF has one (exact); otherwise its first pages rendered to a
/// fixed size (so a huge page size cannot make a huge image) and read by OCR.
async fn pdf_table(bytes: &[u8]) -> Result<Table, IntakeError> {
    let scratch = Scratch::new()?;
    let input = scratch.path("input.pdf");
    tokio::fs::write(&input, bytes)
        .await
        .map_err(|_| IntakeError::Failed)?;
    let tsv = scratch.path("text.tsv");
    let pages = MAX_OCR_PAGES.to_string();
    run_tool(
        &tool("PROCURE_PDFTOTEXT", "pdftotext"),
        &[
            "-tsv".as_ref(),
            "-l".as_ref(),
            pages.as_ref(),
            "-enc".as_ref(),
            "UTF-8".as_ref(),
            input.as_os_str(),
            tsv.as_os_str(),
        ],
    )
    .await
    // A damaged or encrypted PDF that poppler cannot open.
    .map_err(|_| IntakeError::Unsupported)?;
    let words = procure_domain::ocr_table::parse_tsv(&read_tsv(&tsv).await?);
    let letters: usize = words
        .iter()
        .map(|w| w.text.chars().filter(|c| c.is_alphanumeric()).count())
        .sum();
    if letters >= 20 {
        return Ok(words_table(&words, false));
    }
    let tesseract = tool("PROCURE_TESSERACT", "tesseract");
    let pdftoppm = tool("PROCURE_PDFTOPPM", "pdftoppm");
    if !installed(&tesseract) || !installed(&pdftoppm) {
        return Err(IntakeError::OcrUnavailable);
    }
    let prefix = scratch.path("page");
    run_tool(
        &pdftoppm,
        &[
            // Longest side 3508 px: an A4 page at 300 dpi.
            "-scale-to".as_ref(),
            "3508".as_ref(),
            "-gray".as_ref(),
            "-png".as_ref(),
            "-l".as_ref(),
            pages.as_ref(),
            input.as_os_str(),
            prefix.as_os_str(),
        ],
    )
    .await?;
    let mut images: Vec<PathBuf> = std::fs::read_dir(&scratch.0)
        .map_err(|_| IntakeError::Failed)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("page") && n.ends_with(".png"))
        })
        .collect();
    if images.is_empty() {
        return Err(IntakeError::Empty);
    }
    // page-01.png … page-10.png: zero-padded by pdftoppm, so name order is page order.
    images.sort();
    ocr(&scratch, &tesseract, &images).await
}

async fn ocr_image(
    bytes: &[u8],
    kind: &'static str,
    tesseract: &Path,
) -> Result<Table, IntakeError> {
    let pages = image_pages(bytes, kind).ok_or(IntakeError::Unsupported)?;
    if pages.is_empty() {
        return Err(IntakeError::Unsupported);
    }
    if pages.len() > MAX_OCR_PAGES
        || pages
            .iter()
            .any(|(w, h)| *w == 0 || *h == 0 || u64::from(*w) * u64::from(*h) > MAX_OCR_PIXELS)
    {
        return Err(IntakeError::TooLarge);
    }
    if !installed(tesseract) {
        return Err(IntakeError::OcrUnavailable);
    }
    let scratch = Scratch::new()?;
    let input = scratch.path(&format!("scan.{kind}"));
    tokio::fs::write(&input, bytes)
        .await
        .map_err(|_| IntakeError::Failed)?;
    ocr(&scratch, tesseract, &[input]).await
}

/// Japanese + English OCR of the images (each may be a multi-page TIFF), one uniform block
/// of text per page so table rows stay whole lines.
async fn ocr(
    scratch: &Scratch,
    tesseract: &Path,
    images: &[PathBuf],
) -> Result<Table, IntakeError> {
    let list = scratch.path("pages.txt");
    let mut names = String::new();
    for i in images {
        names.push_str(i.to_str().ok_or(IntakeError::Failed)?);
        names.push('\n');
    }
    tokio::fs::write(&list, names)
        .await
        .map_err(|_| IntakeError::Failed)?;
    let base = scratch.path("ocr");
    run_tool(
        tesseract,
        &[
            list.as_os_str(),
            base.as_os_str(),
            "-l".as_ref(),
            "jpn+eng".as_ref(),
            "--psm".as_ref(),
            "6".as_ref(),
            "-c".as_ref(),
            "tessedit_create_tsv=1".as_ref(),
            "-c".as_ref(),
            "tessedit_create_txt=0".as_ref(),
        ],
    )
    .await?;
    let words = procure_domain::ocr_table::parse_tsv(&read_tsv(&scratch.path("ocr.tsv")).await?);
    if words.is_empty() {
        return Err(IntakeError::Empty);
    }
    Ok(words_table(&words, true))
}

/// PNG, JPEG or TIFF (the FAX format), by content.
pub fn image_kind(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if b.starts_with(b"II*\0") || b.starts_with(b"MM\0*") {
        Some("tif")
    } else {
        None
    }
}

/// Width × height of each page, read from the headers before any decoder runs, so a
/// small file declaring an enormous image (or hundreds of FAX pages) is refused up front.
/// `None` when the headers cannot be read.
pub fn image_pages(b: &[u8], kind: &str) -> Option<Vec<(u32, u32)>> {
    let be32 = |o: usize| Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?));
    let be16 = |o: usize| Some(u16::from_be_bytes(b.get(o..o + 2)?.try_into().ok()?));
    match kind {
        // The IHDR chunk comes first.
        "png" if b.get(12..16)? == b"IHDR" => Some(vec![(be32(16)?, be32(20)?)]),
        "jpg" => {
            // Walk the segments to the first start-of-frame marker.
            let mut o = 2;
            loop {
                if *b.get(o)? != 0xFF {
                    return None;
                }
                let marker = *b.get(o + 1)?;
                if marker == 0xFF {
                    o += 1;
                    continue;
                }
                let len = usize::from(be16(o + 2)?);
                let sof = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
                if sof {
                    return Some(vec![(u32::from(be16(o + 7)?), u32::from(be16(o + 5)?))]);
                }
                if len < 2 {
                    return None;
                }
                o += 2 + len;
            }
        }
        "tif" => {
            let little = b.starts_with(b"II");
            let u16_at = |o: usize| {
                let v: [u8; 2] = b.get(o..o + 2)?.try_into().ok()?;
                Some(if little {
                    u16::from_le_bytes(v)
                } else {
                    u16::from_be_bytes(v)
                })
            };
            let u32_at = |o: usize| {
                let v: [u8; 4] = b.get(o..o + 4)?.try_into().ok()?;
                Some(if little {
                    u32::from_le_bytes(v)
                } else {
                    u32::from_be_bytes(v)
                })
            };
            let mut pages = Vec::new();
            let mut ifd = u32_at(4)? as usize;
            while ifd != 0 {
                // One more than allowed is enough to refuse; also stops offset loops.
                if pages.len() > MAX_OCR_PAGES {
                    break;
                }
                let entries = usize::from(u16_at(ifd)?);
                let (mut w, mut h) = (0u32, 0u32);
                for e in 0..entries {
                    let at = ifd + 2 + e * 12;
                    let tag = u16_at(at)?;
                    let value = match u16_at(at + 2)? {
                        3 => u32::from(u16_at(at + 8)?),
                        4 => u32_at(at + 8)?,
                        _ => continue,
                    };
                    match tag {
                        256 => w = value,
                        257 => h = value,
                        _ => {}
                    }
                }
                pages.push((w, h));
                ifd = u32_at(ifd + 2 + entries * 12)? as usize;
            }
            Some(pages)
        }
        _ => None,
    }
}

fn cell_text(cell: &calamine::Data) -> String {
    use calamine::Data;
    match cell {
        Data::String(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        // Shortest text that reads back as the same number (12.5, 0.85, 1000).
        Data::Float(f) => f.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(d) => {
            let (y, m, dd, ..) = d.to_ymd_hms_milli();
            format!("{y:04}-{m:02}-{dd:02}")
        }
        Data::DateTimeIso(s) => s.chars().take(10).collect(),
        Data::DurationIso(_) | Data::Error(_) | Data::Empty => String::new(),
    }
}

fn sheet_rows(bytes: &[u8]) -> Result<Vec<Vec<String>>, IntakeError> {
    use calamine::Reader;
    let mut book = calamine::open_workbook_auto_from_rs(std::io::Cursor::new(bytes.to_vec()))
        .map_err(|_| IntakeError::Unsupported)?;
    let mut first: Option<Vec<Vec<String>>> = None;
    for name in book.sheet_names().to_vec() {
        let Ok(range) = book.worksheet_range(&name) else {
            continue;
        };
        if range.height() > MAX_TABLE_ROWS + 20 {
            return Err(IntakeError::TooLarge);
        }
        let rows: Vec<Vec<String>> = range
            .rows()
            .map(|r| r.iter().take(MAX_TABLE_COLUMNS).map(cell_text).collect())
            .collect();
        if procure_domain::history::find_header(&rows).is_some() {
            return Ok(rows);
        }
        first.get_or_insert(rows);
    }
    first.ok_or(IntakeError::Empty)
}

/// RFC 4180 CSV (quoted fields, doubled quotes, newlines inside quotes); tab-separated when
/// the first line has tabs and no commas.
fn delimited_rows(text: &str) -> Result<Vec<Vec<String>>, IntakeError> {
    let first_line = text.lines().next().unwrap_or("");
    let sep = if first_line.contains('\t') && !first_line.contains(',') {
        '\t'
    } else {
        ','
    };
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            c if c == sep => {
                if row.len() < MAX_TABLE_COLUMNS {
                    row.push(std::mem::take(&mut field));
                }
                field.clear();
            }
            '\r' => {}
            '\n' => {
                if row.len() < MAX_TABLE_COLUMNS {
                    row.push(std::mem::take(&mut field));
                }
                field.clear();
                rows.push(std::mem::take(&mut row));
                if rows.len() > MAX_TABLE_ROWS + 20 {
                    return Err(IntakeError::TooLarge);
                }
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    Ok(rows)
}

fn decode_text(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::SHIFT_JIS.decode(bytes).0.into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn purchase_tables_are_read_from_csv() {
        let csv = "日付,品名,単価\r\n2026/1/5,\"ボルト, M8\",12.5\r\n2026/1/6,\"say \"\"hi\"\"\nline 2\",3\n";
        let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode(csv);
        for bytes in [csv.as_bytes(), &sjis] {
            let rows = table_rows("history.csv", bytes).await.unwrap().rows;
            assert_eq!(rows.len(), 3);
            assert_eq!(rows[1], vec!["2026/1/5", "ボルト, M8", "12.5"]);
            assert_eq!(rows[2][1], "say \"hi\"\nline 2");
        }
        let rows = table_rows("h.tsv", b"date\titem\tprice\n2026-01-05\tnut\t3")
            .await
            .unwrap()
            .rows;
        assert_eq!(rows[1], vec!["2026-01-05", "nut", "3"]);
        assert_eq!(
            table_rows("h.csv", b",,\n,,\n").await,
            Err(IntakeError::Empty)
        );
        assert_eq!(
            table_rows("h.pdf", b"%PDF-1.4").await,
            Err(IntakeError::Unsupported)
        );
        assert_eq!(
            table_rows("h.docx", b"PK\x03\x04").await,
            Err(IntakeError::Unsupported)
        );
        let big = "a\n".repeat(MAX_TABLE_ROWS + 50);
        assert_eq!(
            table_rows("h.csv", big.as_bytes()).await,
            Err(IntakeError::TooLarge)
        );
    }

    #[test]
    fn audio_is_recognised_by_content() {
        assert_eq!(audio_kind(b"RIFF\0\0\0\0WAVEfmt "), Some("wav"));
        assert_eq!(audio_kind(b"\0\0\0\x20ftypM4A \0\0"), Some("m4a"));
        assert_eq!(audio_kind(b"OggS\0\x02\0\0\0\0\0\0"), Some("ogg"));
        assert_eq!(audio_kind(b"ID3\x04\0\0\0\0\0\0\0\0"), Some("mp3"));
        assert_eq!(audio_kind(b"%PDF-1.7 not audio"), None);
        assert_eq!(audio_kind(b"MZ\x90\0\x03\0\0\0\x04\0\0\0"), None);
    }

    #[tokio::test]
    async fn csv_in_shift_jis_and_utf8_is_read() {
        let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode("型番,品名\n6204-2RS,深溝玉軸受\n");
        let t = document_text("catalogue.csv", &sjis).await.unwrap();
        assert!(t.contains("深溝玉軸受") && t.contains("6204-2RS"));
        let t = document_text("a.txt", "\u{feff}型番: HG-KR43".as_bytes())
            .await
            .unwrap();
        assert_eq!(t, "型番: HG-KR43");
        assert_eq!(
            document_text("x.exe", b"MZ...").await,
            Err(IntakeError::Unsupported)
        );
        assert_eq!(
            document_text("e.csv", b" , ,\n").await,
            Err(IntakeError::Empty)
        );
    }

    #[test]
    fn huge_or_bomb_workbooks_are_refused() {
        assert_eq!(dimension_cells(r#"<dimension ref="A1:C10"/>"#), 30);
        assert!(dimension_cells(r#"<dimension ref="A1:XFD1048576"/>"#) > MAX_SHEET_CELLS);
        // A zip whose one entry inflates to more than the limit.
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            w.start_file(
                "xl/sharedStrings.xml",
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
            let zeros = vec![b'0'; 1024 * 1024];
            for _ in 0..45 {
                std::io::Write::write_all(&mut w, &zeros).unwrap();
            }
            w.finish().unwrap();
        }
        let bomb = buf.into_inner();
        assert!(bomb.len() < 1024 * 1024, "small on disk");
        assert_eq!(check_workbook(&bomb), Err(IntakeError::Unsupported));
    }

    const SCAN: &[&str] = &[
        "2024/01/15",
        "山田商事",
        "六角ボルト",
        "M8-30",
        "100",
        "12.5",
    ];

    /// Real tesseract (jpn+eng) on a scan rendered from a Japanese purchase table: as a PNG,
    /// a FAX-style 1-bit TIFF and an image-only PDF.
    #[tokio::test]
    async fn scanned_purchase_records_are_read_by_ocr() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        for name in [
            "purchase-scan.png",
            "purchase-scan.tif",
            "purchase-scan.pdf",
        ] {
            let bytes = std::fs::read(dir.join(name)).unwrap();
            let t = table_rows(name, &bytes).await.unwrap();
            assert!(t.ocr, "{name}");
            let header = procure_domain::history::find_header(&t.rows).expect(name);
            assert_eq!(t.rows[header][0], "日付", "{name}");
            let row = &t.rows[header + 1];
            for (i, want) in SCAN.iter().enumerate() {
                assert!(
                    row[i].replace(' ', "") == *want,
                    "{name} column {i}: {:?}",
                    row
                );
            }
            assert_eq!(t.rows[header + 3][4], "200", "{name}");
            assert_eq!(t.unclear.len(), t.rows.len());
        }
    }

    #[tokio::test]
    async fn pdf_text_layers_become_tables_without_ocr() {
        let pdf = minimal_pdf("HG-KR43 servo bearing 6204ZZ quantity 20");
        let t = table_rows("record.pdf", &pdf).await.unwrap();
        assert!(!t.ocr);
        assert!(t.unclear.is_empty());
        assert!(t.rows.concat().join(" ").contains("6204ZZ"), "{:?}", t.rows);
    }

    #[tokio::test]
    async fn scans_are_refused_before_decoding_when_too_large_or_ocr_is_missing() {
        // A 100-byte PNG declaring 20000 × 20000 pixels.
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend(20_000u32.to_be_bytes());
        png.extend(20_000u32.to_be_bytes());
        png.extend([8, 0, 0, 0, 0]);
        assert_eq!(image_pages(&png, "png"), Some(vec![(20_000, 20_000)]));
        assert_eq!(table_rows("x.png", &png).await, Err(IntakeError::TooLarge));
        // A JPEG: APP0 segment, then a baseline frame of 640 × 480.
        let jpg = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x01,
            0xE0, 0x02, 0x80, 0x01, 0x01, 0x11, 0x00,
        ];
        assert_eq!(image_pages(&jpg, "jpg"), Some(vec![(640, 480)]));
        assert_eq!(image_pages(&jpg[..9], "jpg"), None);
        // A little-endian TIFF whose IFD chain points back at itself (a FAX with "endless"
        // pages): refused, not looped over.
        let mut tif = b"II*\0\x08\0\0\0".to_vec();
        tif.extend(2u16.to_le_bytes());
        for (tag, v) in [(256u16, 1728u32), (257, 2200)] {
            tif.extend(tag.to_le_bytes());
            tif.extend(4u16.to_le_bytes());
            tif.extend(1u32.to_le_bytes());
            tif.extend(v.to_le_bytes());
        }
        tif.extend(8u32.to_le_bytes());
        let pages = image_pages(&tif, "tif").unwrap();
        assert_eq!(pages[0], (1728, 2200));
        assert!(pages.len() > MAX_OCR_PAGES);
        assert_eq!(
            table_rows("fax.tif", &tif).await,
            Err(IntakeError::TooLarge)
        );
        assert_eq!(image_pages(b"II*\0\xff\xff\0\0", "tif"), None);

        let scan = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/purchase-scan.png"),
        )
        .unwrap();
        assert_eq!(
            ocr_image(&scan, "png", Path::new("/nonexistent/tesseract")).await,
            Err(IntakeError::OcrUnavailable)
        );
    }

    #[tokio::test]
    async fn pdf_text_layer_is_read() {
        // A minimal one-page PDF with the text "HG-KR43 servo".
        let pdf = minimal_pdf("HG-KR43 servo");
        let t = document_text("datasheet.pdf", &pdf).await.unwrap();
        assert!(t.contains("HG-KR43"), "{t}");
    }

    /// Real ffmpeg; a stand-in for whisper.cpp that prints a fixed transcript (and checks it
    /// was given a 16 kHz WAV and the model).
    #[tokio::test]
    async fn voice_notes_are_converted_and_length_limited() {
        let dir = Scratch::new().unwrap();
        let fake = dir.path("whisper.sh");
        std::fs::write(
            &fake,
            "#!/bin/sh\n[ \"$1\" = -m ] && [ \"$2\" = model.bin ] && head -c 4 \"$4\" | grep -q RIFF && echo '型番 6204-2RS の軸受です'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let t = CommandTranscriber {
            ffmpeg: "ffmpeg".into(),
            whisper: fake,
            model: "model.bin".into(),
        };
        let tone = |secs: u32| {
            let out = dir.path(&format!("tone{secs}.ogg"));
            let ok = std::process::Command::new("ffmpeg")
                .args(["-nostdin", "-loglevel", "error", "-f", "lavfi", "-i"])
                .arg(format!("sine=frequency=440:duration={secs}"))
                .args(["-c:a", "libopus"])
                .arg(&out)
                .status()
                .unwrap()
                .success();
            assert!(ok, "ffmpeg available");
            std::fs::read(out).unwrap()
        };
        let audio = tone(2);
        assert_eq!(audio_kind(&audio), Some("ogg"));
        assert_eq!(
            t.transcribe(&audio, "ogg", Some("ja")).await.unwrap(),
            "型番 6204-2RS の軸受です"
        );
        assert_eq!(
            t.transcribe(&tone(MAX_AUDIO_SECONDS + 5), "ogg", None)
                .await,
            Err(IntakeError::TooLong)
        );
        assert_eq!(
            t.transcribe(b"OggS but not really audio", "ogg", None)
                .await,
            Err(IntakeError::Failed)
        );
    }

    fn minimal_pdf(text: &str) -> Vec<u8> {
        let content = format!("BT /F1 12 Tf 50 700 Td ({text}) Tj ET");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for off in offsets {
            out.extend(format!("{off:010} 00000 n \n").bytes());
        }
        out.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .bytes(),
        );
        out
    }
}

/// Attachments read from one supplier email.
pub const MAX_EMAIL_ATTACHMENTS: usize = 5;

/// Text of a supplier's reply to a request for quotation, as it arrived: a saved email
/// (`.eml`, with its PDF / Excel / scanned attachments read too), a PDF (text layer, else
/// OCR), an Excel or CSV sheet, or a scanned page / FAX image (OCR).
pub async fn quote_reply_text(file_name: &str, bytes: &[u8]) -> Result<String, IntakeError> {
    let ext = Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let text = if ext == "eml" {
        email_text(bytes).await?
    } else {
        file_text(file_name, bytes).await?
    };
    let text = clip(text.trim());
    if text.chars().filter(|c| c.is_alphanumeric()).count() < 3 {
        return Err(IntakeError::Empty);
    }
    Ok(text)
}

/// One document: its text layer, or OCR read back as tab-separated lines.
async fn file_text(file_name: &str, bytes: &[u8]) -> Result<String, IntakeError> {
    if image_kind(bytes).is_none() {
        match document_text(file_name, bytes).await {
            Err(IntakeError::Empty) if bytes.starts_with(b"%PDF-") => {}
            other => return other,
        }
    }
    let table = table_rows(file_name, bytes).await?;
    Ok(table
        .rows
        .iter()
        .map(|r| r.join("\t").trim_end().to_string())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n"))
}

async fn email_text(bytes: &[u8]) -> Result<String, IntakeError> {
    use mail_parser::MimeHeaders;
    let msg = mail_parser::MessageParser::default()
        .parse(bytes)
        .ok_or(IntakeError::Unsupported)?;
    let mut out = String::new();
    if let Some(subject) = msg.subject() {
        out.push_str(&format!("Subject: {subject}\n"));
    }
    if let Some(from) = msg.from().and_then(|a| a.first()) {
        let name = from.name().unwrap_or("");
        let addr = from.address().unwrap_or("");
        out.push_str(&format!("From: {name} <{addr}>\n"));
    }
    if let Some(date) = msg.date() {
        out.push_str(&format!("Date: {}\n", date.to_rfc3339()));
    }
    out.push('\n');
    if let Some(body) = msg.body_text(0) {
        out.push_str(body.trim());
    } else if let Some(html) = msg.body_html(0) {
        out.push_str(strip_tags(&html).trim());
    }
    for part in msg.attachments().take(MAX_EMAIL_ATTACHMENTS) {
        let name = part.attachment_name().unwrap_or("attachment");
        // An unreadable attachment (a logo, a zip) is noted, not fatal: the body may be
        // enough, and the operator sees what was skipped.
        let text = match Box::pin(file_text(name, part.contents())).await {
            Ok(t) => t,
            Err(IntakeError::OcrUnavailable) => "(scanned; OCR is not installed)".to_string(),
            Err(_) => "(not readable)".to_string(),
        };
        out.push_str(&format!("\n\n--- Attachment: {name} ---\n{}", text.trim()));
    }
    Ok(out)
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

#[cfg(test)]
mod quote_reply_tests {
    use super::*;

    #[tokio::test]
    async fn iso_2022_jp_email_with_csv_attachment() {
        // 「お見積り」 in ISO-2022-JP (the usual Japanese business mail charset).
        let subject = "=?ISO-2022-JP?B?GyRCJCo4K0BRJGobKEI=?=";
        let body = "\x1b$B8+@Q$j$^$9\x1b(B 6204ZZ 1180";
        let csv = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            "品番,単価\n6204ZZ,1180\n".as_bytes(),
        );
        let eml = format!(
            "From: Sales <sales@bearing.test>\r\nSubject: {subject}\r\nMIME-Version: 1.0\r\n\
             Content-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n--b1\r\n\
             Content-Type: text/plain; charset=ISO-2022-JP\r\n\r\n{body}\r\n--b1\r\n\
             Content-Type: text/csv; name=\"quote.csv\"\r\n\
             Content-Disposition: attachment; filename=\"quote.csv\"\r\n\
             Content-Transfer-Encoding: base64\r\n\r\n{csv}\r\n--b1--\r\n"
        );
        let text = quote_reply_text("reply.eml", eml.as_bytes()).await.unwrap();
        assert!(text.contains("Subject: お見積り"), "{text}");
        assert!(text.contains("From: Sales <sales@bearing.test>"));
        assert!(text.contains("見積ります 6204ZZ 1180"), "{text}");
        assert!(text.contains("--- Attachment: quote.csv ---\n品番,単価\n6204ZZ,1180"), "{text}");
    }

    #[tokio::test]
    async fn plain_files_and_rejections() {
        let t = quote_reply_text("q.csv", "item,price\n6204ZZ,1180\n".as_bytes()).await;
        assert_eq!(t.unwrap(), "item,price\n6204ZZ,1180");
        assert!(matches!(
            quote_reply_text("q.bin", &[0, 1, 2, 3]).await,
            Err(IntakeError::Unsupported)
        ));
        assert!(matches!(
            quote_reply_text("q.eml", b"").await,
            Err(IntakeError::Unsupported | IntakeError::Empty)
        ));
    }

    #[test]
    fn html_bodies_lose_their_tags() {
        assert_eq!(strip_tags("<p>単価&nbsp;<b>1,180</b></p>").trim(), "単価  1,180");
    }
}
