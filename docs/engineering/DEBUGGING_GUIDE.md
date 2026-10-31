# Debugging guide

Rule: **reproduce → locate → root-cause → smallest fix → re-run failing test → re-run related tests → record.** Owner: `.claude/agents/debugger.md`.

## Forbidden "fixes"
Hiding errors, `#[ignore]`/skipping tests, removing validation, swallowing errors, sleeps, swapping real code for mocks, weakening security.

## Signals
| Layer | How |
|---|---|
| Rust server | `RUST_LOG=procure=debug,tower_http=debug`, JSON logs via `tracing`; every request gets `x-request-id` (generated if absent) recorded on the span and returned in the response and error body |
| Rust panics/crashes | `RUST_BACKTRACE=1` (full: `=full`); `rust-lldb` / `rust-gdb` on the binary |
| SQL | SQLx errors carry the constraint name; Postgres `log_min_error_statement=error` in dev |
| API diagnostics | Request/response logging middleware redacts `authorization`, `cookie`, `password*`, `token*`, and file bodies |
| Desktop Rust layer | Tauri log plugin → stdout in dev, rotating file in user log dir in release (info level, redacted) |
| Desktop UI | Global error boundary + `window.onerror`/`unhandledrejection` → forwarded to Rust log with the correlation ID; WebKit inspector **dev builds only** (`devtools` feature off in release; no macOS private API) |
| Tests | `cargo test -- --nocapture`; WebdriverIO screenshots + logs to `reports/e2e/`; Playwright traces |
| Gate | `scripts/gate.sh` logs every step to `reports/gate-<ts>-<step>.log` |

Correlation: UI action → IPC command (span with `cid`) → HTTP header `x-request-id: cid` → server span → DB/job spans carry `cid`; jobs record the originating `cid`.

## Bug reports
Significant defects: `docs/engineering/bugs/BUG-YYYYMMDD-<slug>.md` with: symptom, reproduction command, observed output, root cause, fix commit, regression test name, residual uncertainty.

## Worked example (2026-10-31)
Symptom: gate passed while a GitHub token sat in an uncommitted file. Repro: canary repo with planted token. Root cause: `gitleaks detect` scans git history only. Fix: gate runs `gitleaks dir` (tree) and `gitleaks git` (history). Verified: canary now fails `gitleaks-tree`; clean repo passes.
