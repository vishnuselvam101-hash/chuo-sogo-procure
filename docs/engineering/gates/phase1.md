# Quality gate — Phase 1 (foundation), 2026-10-31

## Automated
`scripts/gate.sh full`: ui-format, ui-typecheck, ui-lint, ui-test, ui-build, fmt, clippy (-D warnings), test, cargo-audit, pnpm-audit (prod), gitleaks (tree + history), semgrep (p/rust, p/typescript, p/secrets), trivy fs — **all PASS** on the commit that adds this file.

| Suite | Count | Result |
|---|---|---|
| `procure-domain` unit (money, RBAC matrix) | 12 | pass |
| `procure-server` unit (password, tokens, rate limiter) | 4 | pass |
| `procure-server` integration (real PostgreSQL, runtime role) | 18 | pass |
| `procure-desktop` unit (URL/path/method validation, log sanitising) | 5 | pass |
| OpenAPI contract (`tests/openapi.rs`) | 2 | pass |
| Desktop UI (Vitest: i18n parity/fallback/key coverage, formatting, shell, language persistence, role nav) | 14 | pass |
| Desktop E2E (`scripts/e2e-desktop.sh`, release binary via tauri-driver/WebKitWebDriver on Xvfb) | 6 | pass |

E2E checks: app identity, real login against backend, Japanese navigation, operations workspace for staff, instant switch to Tamil, Tamil persists across app restart, Hindi renders. Screenshots: `reports/e2e-smoke/` (local, not committed).

## Packaging
- Linux `.deb` (5.2 MB) and AppImage (84 MB) built on Ubuntu 26.04 with `cargo tauri build`. The E2E run used the same release binary. Installing the `.deb` system-wide was not tested.
- First AppImage attempt failed on a network timeout fetching `AppRun` from GitHub; the retry succeeded.
- Windows/macOS: workflow `desktop-build` exists, **not run yet**. Nothing has been signed.

## Reviews
- Independent security review of the foundation (cybersecurity-expert brief, ASVS 5.0.0 lens): 0 critical; SF-004 (High), SF-005..SF-007 (Medium), SF-008/009 (Low), SF-010 (Info). All fixed and covered by tests; see `docs/security/SECURITY_FINDINGS.md`.
- Independent desktop security review (IPC, CSP, token handling, URL validation): 0 critical/high; SF-013/014 (Medium), SF-015..018 (Low), SF-019 (Info). All fixed; desktop unit tests 5, E2E re-run 6/6 on the rebuilt release binary.
- API contract: `api/openapi.json` generated from the handlers (utoipa); drift tests in Rust (`tests/openapi.rs`) and TypeScript (`gen:api:check`); UI types are generated, not hand-written.

## Defects found during the phase
1. Gate missed uncommitted secrets → `gitleaks dir` added (SF-001).
2. Path/JSON rejections returned plain text → JSON error extractors (SF-002).
3. Concurrent demotion could leave an org with zero admins → per-org lock (SF-003).
4. Request ids were missing from server logs → span field added.
5. The gitleaks tree scan flagged compiled `.rmeta` files in `target/` → `.gitleaks.toml` allowlists build output only. A canary token planted in `src/` is still detected.
6. Headless screenshots via X capture came out black → switched to WebDriver screenshots (the app was rendering correctly).
