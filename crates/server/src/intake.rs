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
}

impl IntakeError {
    pub const fn code(&self) -> &'static str {
        match self {
            IntakeError::Unsupported => "file_type",
            IntakeError::TooLong => "audio_too_long",
            IntakeError::Empty => "nothing_readable",
            IntakeError::SpeechUnavailable => "speech_unavailable",
            IntakeError::Failed => "intake_failed",
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
