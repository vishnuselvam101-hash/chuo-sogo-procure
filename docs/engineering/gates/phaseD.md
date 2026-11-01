# Quality gate — Phase D (AI listing assistance), 2026-11-01

## Scope
- **AI capabilities** (`procure_ai::listing`): `extract_listing` (manufacturer, model number,
  condition, Japanese/English title and description, specifications) and `translate_listing`
  (title/description draft in the other language). Same rules as Phase 3: fenced untrusted
  input, strict schema, deterministic guards — model numbers must appear as whole words
  (Japanese text without spaces handled), specification numbers must appear in the source
  (full-width digits normalised; other numeral systems not trusted), Latin manufacturer names
  as whole words; flags + confidence caps; one language marked `machine_translation`.
- **Inputs** (`intake.rs`, all on this server): typed notes; **voice notes** (wav/mp3/m4a/
  ogg/webm/flac, ≤ 5 min, ≤ 10 MB) → `ffmpeg` (pinned demuxer, local file protocol only) →
  whisper.cpp-compatible command (`PROCURE_STT_COMMAND`, `PROCURE_STT_MODEL`); raw audio is
  never stored; **catalogue files**: PDF text layer (`pdftotext`, 30 pages), Excel `.xlsx`
  (zip-bomb and sheet-size checks before parsing, separate thread), CSV/TXT in UTF-8 or
  Shift_JIS. Tool output capped, timeouts, private scratch directory (`PROCURE_WORK_DIR`).
- **API** (supplier, `manage_listings`): from-text, from-voice, from-file, translate, decision;
  suggestions recorded in `ai_suggestions` with `input_source`; every call audited;
  Chuo-Sogo's per-organisation AI switch applies.
- **Desktop**: AI assistant panel in the listing editor (type / voice note — record where the
  webview allows, or upload — / catalogue file), shows the transcript or read text, flags
  and unconfirmed values; "Apply" fills **empty fields only**; "Draft the other language".
  Nothing is saved until the supplier saves, and Chuo-Sogo still reviews every listing.

## Automated
`scripts/gate.sh full` — all 17 steps **PASS**.

| Suite | Count | Result |
|---|---|---|
| AI listing guards + mock (incl. full-width digits, manufacturer whole words) | 6 + 1 | pass |
| Intake: audio detection, Shift_JIS CSV, PDF text, voice conversion with real ffmpeg and 5-minute limit, zip-bomb / huge-sheet refusal | 5 | pass |
| Server listing AI integration (typed, file, voice with a stand-in recogniser, translate, isolation, AI switch) | 4 | pass |
| Desktop UI (Vitest) | 40 | pass |
| Catalogue E2E: supplier fills a listing from a **real voice note** (ffmpeg conversion; whisper stand-in) and a Shift_JIS CSV catalogue, adds photos, submits → admin approves → customer finds it | 13 checks | **13/13 pass** |
| Admin E2E / business workflow E2E (regression) | 20 / 30 checks | **pass** |

The E2E found a real bug before review: a second suggestion overwrote the product name from
the first; applying now fills empty fields only (regression-tested).

## Review
Local independent review: no command/argument injection, no tenant-isolation gap. Fixed:
SF-051 (spreadsheet zip bomb / huge declared sheet; unbounded tool output), SF-052
(full-width or other numerals bypassed the number guard; manufacturer substring match),
SF-053 (translation draft overwrote supplier text; recorded as applied before the decision
succeeded), SF-054 (ffmpeg could pick playlist/concat demuxers — demuxer pinned, file
protocol only), plus Semgrep: scratch space moved from the shared temp directory to an
owner-only work directory created atomically.

## Not done / decisions
- **Speech-to-text is not installed on this server**: building whisper.cpp was blocked by
  the auto-mode policy (code from an external repository). Voice notes return
  `speech_unavailable` until the founder installs whisper.cpp and sets
  `PROCURE_STT_COMMAND`/`PROCURE_STT_MODEL` (a multilingual `ggml-small.bin` model already
  exists on this server under /root/vnx-studio/models).
- **Photos → fields** (reading nameplates) needs a vision-capable model: decision **[D5]**
  (local vision model vs an approved paid provider). Not built.
- Production AI provider is still the offline mock (`PROCURE_AI_PROVIDER=mock`); real
  quality needs a local or approved model.
