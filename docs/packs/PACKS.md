# Content packs

Chuo-Sogo Procure ships its offline content as **packs**. `packs/manifest.json` is the single source of truth: every file has a path, size and SHA-256, and large files also have the official upstream URL.

- **core** packs go inside the installer. Their `url` is `null`; `upstream_url` says where the build server got them from.
- **large** packs are downloaded by the app on first run from the official upstream `url` and checked against `sha256`.

## Packs

| id | kind | version | size (bytes) | size | licence | source |
|---|---|---|---:|---:|---|---|
| ai-model, `standard` variant (default, ≥16 GB RAM) | large | Qwen3-14B Q4_K_M | 9,001,764,504 | 9.00 GB | Apache-2.0 | huggingface.co/Qwen/Qwen3-14B-GGUF @530227a |
| ai-model, `light` variant (≥8 GB RAM) | large | Qwen3-8B Q4_K_M | 5,027,795,032 | 5.03 GB | Apache-2.0 | huggingface.co/Qwen/Qwen3-8B-GGUF @7c41481 |
| voice-model | large | whisper large-v3-turbo | 1,624,556,353 | 1.62 GB | MIT | huggingface.co/ggerganov/whisper.cpp @5359861 |
| ai-runtime | core | llama.cpp b11960 (= v0.8.0) | 63,560,107 (3 platforms) | 63.6 MB | MIT | github.com/ggml-org/llama.cpp releases |
| voice-runtime | core | whisper.cpp v1.9.5 (b5454) | 19,293,913 (2 platforms) | 19.3 MB | MIT | github.com/ggml-org/whisper.cpp releases |
| ocr-data | core | tessdata_best @e12c65a | 61,990,457 | 62.0 MB | Apache-2.0 | github.com/tesseract-ocr/tessdata_best |
| fonts | core | google/fonts @2fe1e8c | 23,172,990 | 23.2 MB | OFL-1.1 | google/fonts `ofl/notosansjp`, `ofl/notoserifjp` |
| item-dictionary | core | 1.0.0 | 314,775 | 0.3 MB | Chuo-Sogo (own work) | `packs/item-dictionary/` |

Every licence was checked for redistribution and commercial use: Apache-2.0 (Qwen3 GGUF repos ship the LICENSE file; tessdata_best), MIT (llama.cpp, whisper.cpp, Whisper weights via the ggerganov/whisper.cpp model card), and OFL-1.1 (Noto, which may be bundled with software but not sold on its own). Each pack carries its licence file, and the installer must keep those files next to the content.

### Totals

| | bytes | |
|---|---:|---|
| core (all platforms) | 348,222,066 | 348 MB |
| core in one Windows installer (only the Windows archives) | 298,706,587 | 299 MB |
| large, standard (14B + voice) | 10,626,320,857 | 10.63 GB |
| large, light (8B + voice) | 6,652,351,385 | 6.65 GB |
| **installed total, standard** (core all platforms + large) | **10,974,542,923** | **10.97 GB** |
| installed total, light | 7,000,573,451 | 7.00 GB |

### Per-platform runtime files

| pack | windows-x64 | linux-x64 | macos-arm64 |
|---|---|---|---|
| ai-runtime | `llama-b11960-bin-win-cpu-x64.zip` (CPU build; it loads the best `ggml-cpu-*.dll` for the CPU at runtime, AVX2/haswell included) | `llama-b11960-bin-ubuntu-x64.tar.gz` | `llama-b11960-bin-macos-arm64.tar.gz` (Metal) |
| voice-runtime | `whisper-bin-x64.zip` (`whisper-server.exe`, `whisper-cli.exe`) | `whisper-bin-ubuntu-x64.tar.gz` | **none published upstream.** whisper.cpp releases only an xcframework for Apple, so macOS needs a source build (MIT) |

An installer bundles only its own platform's archive, plus the pack LICENSE.

## Item dictionary

`packs/item-dictionary/items.json` holds 605 MRO items (electrical 175, tools 185, consumables 245) for factory buying in Japan. Each item has a canonical ja/en name, a kana reading, synonyms (abbreviations, colloquial names, and trademarks used as generic words, marked `(商標)`), units, typical pack sizes, the spec attributes a quote must state, and part-number regexes. 52 items have regexes. They are either JIS/ISO/IEC designation formats (bearings 6205ZZ, O-rings P22A, inserts CNMG120408, V-belts B-50, crimp terminals R5.5-6, cable designations VVF2.0-3C) or published series formats from real manufacturers (Mitsubishi Electric NF/NV/S-T/FR/FX, OMRON MY/H3/E5/E2E/E3Z/G3/S8, Fuji Electric, IDEC, SMC). There are no invented companies or brands. 71 items are marked `uncertain` because naming or pack sizes vary between buyers.

- Source: `packs/item-dictionary/build_items.py`. Edit it, then run `python3 -I packs/item-dictionary/build_items.py`.
- Schema: `packs/item-dictionary/schema.json` (JSON Schema 2020-12).
- Check: `python3 -I scripts/packs/validate-dictionary.py`. It checks the schema, unique ids and names, that every regex compiles, and that every regex matches its own example.

## Scripts (build server)

```sh
python3 -I scripts/packs/build-manifest.py   # packs/sources.json + packs/item-dictionary -> packs/manifest.json
scripts/packs/fetch.sh [pack-id ...]          # download/copy into /srv/procure-packs, resumable, idempotent; ends with verify
scripts/packs/verify.sh [pack-id ...]         # size + SHA-256 of every manifest file (PACK_ROOT overrides the root)
```

`packs/sources.json` pins every upstream file by immutable URL (Hugging Face revision hash, GitHub release tag, or git commit). Its SHA-256 values come from the upstream's own digest where one is published (HF LFS oid, GitHub release asset digest); otherwise they were computed once from the pinned URL. `fetch.sh` downloads to `<file>.part` with `curl -C -`, checks for 5 GB of free headroom first, and only renames the file once size and hash match. Binaries are never committed (`packs/.gitignore`).

## How the app should install packs

1. **Pack root:** a per-user data directory, e.g. `%LOCALAPPDATA%\ChuoSogoProcure\packs` on Windows. Store files at `<root>/<path>` exactly as in the manifest.
2. **Core packs:** the installer copies its platform's core files into the root. On first run the app verifies their SHA-256 against the bundled `manifest.json`. Unpack the `ai-runtime` and `voice-runtime` archives into `<root>/<pack-id>/<platform>/`, and keep the archive hash as the integrity record.
3. **Large packs:** fetch only after the user agrees (show the size from the manifest and the free disk space). For `ai-model`, pick the `default` variant (`standard`, 14B) when total RAM ≥ its `min_ram_gb` (16), otherwise `light` (8B), and let the user change it in Settings. Download with HTTP Range resume to `<file>.part`, check size, then SHA-256, then rename atomically. On a mismatch, delete the file and retry up to 3 times, then show an error. Never load a file that failed verification.
4. **Before downloading,** check that free space ≥ file size + 2 GB. Downloads must be cancellable and resumable after a restart.
5. **Updates:** a new manifest `version` for a pack means re-download only the files whose `sha256` changed, then delete the old files.
6. **Licences:** show each pack's `licence` and `licence_url` in Settings → About. Keep each LICENSE/OFL file installed.
7. **Offline / air-gapped sites:** an admin can copy a verified `/srv/procure-packs` tree (or a USB copy) into the pack root, and the app accepts it if every hash matches.

## Full Windows installer and standalone mode

`scripts/demo/build-full-local.sh` builds `reports/builds/<version>/full/ChuoSogo-Setup-<version>.exe` (NSIS, **unsigned**): the Procure app, `procure-server.exe`, a trimmed portable PostgreSQL 18.6 (official EDB Windows x64 binaries, PostgreSQL licence; no pgAdmin, StackBuilder, docs, headers or symbols; the zip is pinned by SHA-256) and the Windows core packs, all under the app's resources in `full/`. Large packs are not inside.

On first run the app asks "use on this PC only" or "connect to a server" (installations without `full/` and older prefs files always mean "server"). In standalone mode the native layer (`apps/desktop/src-tauri/src/local.rs`) verifies and unpacks the core packs into the per-user data dir, runs `initdb` (random passwords in `secrets.json`), starts PostgreSQL, procure-server and, when the AI model is installed, llama-server, all on 127.0.0.1 with free ports, runs `procure-server migrate`, and on first setup creates the local operator, the user's factory and its administrator through the API. Children are killed with the app (Windows job object; kill on drop and a clean `pg_ctl stop` on exit). Logs: `<data>/logs/`. The pack manager (`packs.rs`) installs packs by id only, downloads large packs over HTTPS from the manifest URLs with Range resume, checks free space (size + 2 GB) and verifies size and SHA-256 before recording a file in `installed.json`.

Windows helper tools are core packs in the installer, each pinned by SHA-256 and unpacked on first run: `ocr-runtime` (Tesseract 5.4.0 from the UB Mannheim build, repacked without its installer by `scripts/packs/repack-tesseract.py`, Apache-2.0), `pdf-tools` (Poppler 26.09.0 for pdftoppm/pdftotext, GPL-2.0-or-later, with COPYING and a written source offer) and `media-tools` (FFmpeg 8.1.3 LGPL shared build from BtbN, with a source offer). In local mode the app passes them to the server as `PROCURE_TESSERACT`, `PROCURE_PDFTOPPM`, `PROCURE_PDFTOTEXT`, `PROCURE_FFMPEG` and `TESSDATA_PREFIX` (the ocr-data pack); on Linux and macOS the system's tools on PATH are used.

"Run as administrator": PostgreSQL refuses administrator rights, so the app starts it through `pg_ctl`, which drops them with a restricted token (EDB's approach); the app then works either way. Starting it normally is still recommended.
