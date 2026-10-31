# Security findings

| ID | Date | Title | Severity | ASVS | Status | Evidence |
|---|---|---|---|---|---|---|
| SF-001 | 2026-10-31 | Gate missed secrets in uncommitted files (`gitleaks detect` history-only) | Medium (tooling) | — | Fixed | canary run; `scripts/gate.sh` now runs `gitleaks dir` + `gitleaks git` |
| SF-002 | 2026-10-31 | Malformed path/JSON returned plain-text errors (inconsistent error contract) | Low | V16 | Fixed | `ApiPath/ApiJson/ApiQuery` extractors; test `responses_carry_a_request_id_and_errors_hide_internals` |
| SF-003 | 2026-10-31 | Concurrent mutual demotion could leave an org with zero admins | Medium | V8 | Fixed | per-org advisory lock; test `concurrent_mutual_demotion_never_leaves_an_org_without_admin` |
| SF-004 | 2026-10-31 | Adding an existing account exposed its profile cross-tenant (enumeration, audit side-channel) and let a customer admin give an internal operator approval authority | High | V8.2, V8.3 | Fixed | every add is an invitation the user must accept; accounts are permanently `internal` or `customer` (API + DB trigger); tests `adding_an_existing_account_reveals_nothing_until_it_is_accepted`, `staff_accounts_can_never_hold_customer_roles` |
| SF-005 | 2026-10-31 | No rate limiting; argon2 ran on async workers inside a DB transaction; reset requests could keep invalidating the victim's link | Medium | V6.3, V2 | Fixed | per-IP/per-email fixed-window limits, argon2 in `spawn_blocking` behind a 4-permit semaphore and outside transactions, 5-min reset cooldown; tests `login_is_rate_limited_per_client`, `repeated_reset_requests_do_not_kill_the_first_link` |
| SF-006 | 2026-10-31 | Lockout response (429) revealed account existence and allowed cheap lock-out | Medium | V6.3 | Fixed | locked = same `invalid_credentials` + same argon2 time; threshold 10; test `locked_and_unknown_accounts_are_indistinguishable` |
| SF-007 | 2026-10-31 | Server role owned the tables: a bug/injection could disable RLS or audit triggers | Medium | V14 | Fixed | owner role `procure` (migrations) vs runtime `procure_runtime` (DML only, no UPDATE/DELETE on audit_events); `serve` never migrates; tests run as runtime role and assert ALTER/DROP/DELETE fail |
| SF-008 | 2026-10-31 | Procurement admins reached all customer orgs without assignment/audit (contradicted ADR D4) | Low | V8 | Fixed | all internal roles need an audited assignment; test `internal_admins_also_need_an_assignment` |
| SF-009 | 2026-10-31 | Client-chosen `x-request-id` (arbitrary text) stored in audit log | Low | V16 | Fixed | only UUIDs accepted, else server-generated; test `client_request_ids_must_be_uuids` |
| SF-010 | 2026-10-31 | Test-mode outbox wrote tokens world-readable; mail sent before commit | Info | — | Fixed | files 0600, mail after commit |
| SF-011 | 2026-10-31 | `rsa` RUSTSEC-2023-0071 in Cargo.lock via sqlx optional MySQL driver | Info | V15 | Accepted | not compiled (`cargo tree -i rsa --target all -e all` empty); ignored in `.cargo/audit.toml` with reason; re-check on sqlx feature changes |
| SF-012 | 2026-10-31 | semgrep `rust.lang.security.args.args` on subcommand parsing | Info | — | False positive | argv[0] skipped; inline `nosemgrep` with reason |
| SF-013 | 2026-10-31 | Desktop: percent-encoded dot-segments (`%2e%2e`) escaped the `/api/v1/` path check, so the token could be sent to other paths on the server host | Medium | V8/V50 | Fixed | `validate::api_url` checks origin + prefix after URL normalisation and rejects `%2e`/`%2f`; tests `api_urls_stay_on_origin_and_prefix` |
| SF-014 | 2026-10-31 | Desktop: HTTP client followed redirects (body incl. password re-posted on 307/308; token kept on same-host redirects, even to http) | Medium | V12/V50 | Fixed | `redirect::Policy::none()`; E2E re-run 6/6 |
| SF-015 | 2026-10-31 | Desktop: token not bound to its server (race between login and server change) | Low | V7 | Fixed | token stored with issuing server, sent only there |
| SF-016 | 2026-10-31 | Desktop: `client_log` allowed newline/escape injection and unbounded request ids | Low | V16 | Fixed | control chars replaced, UUID-only request id, level allowlist; test `log_text_strips_control_characters` |
| SF-017 | 2026-10-31 | Desktop: `core:default` granted unused webview permissions (native menus, events, path resolution) | Low | V50 | Fixed | capability lists only the 7 app commands; E2E 6/6 with reduced set |
| SF-018 | 2026-10-31 | CI: installer workflow used a floating tauri-cli version and a shared build cache | Low | V15 | Fixed | `tauri-cli =2.12.4 --locked`, no cache in `desktop-build` |
| SF-019 | 2026-10-31 | Desktop: re-login replaced a token without revoking it server-side; CSP keeps `style-src 'unsafe-inline'` | Info | — | Fixed / Accepted | previous session revoked before new login; inline styles accepted (no inline script allowed) |

## Residual risk (Phase 1)
- Audit hash chain is unkeyed SHA-256; an attacker with DB owner access could rewrite it. Runtime role cannot. External anchoring (periodic head export / HMAC) planned for Phase 6.
- Rate limits are in-process and keyed on peer IP; behind a reverse proxy they need trusted forwarding headers (not implemented).
- Password-reset timing differs slightly between existing and unknown emails (code paths differ); response is identical.
- Admins cannot cancel a pending invitation yet; the invitee can decline.
- `list_audit` re-verifies the whole chain per request (load concern for large logs).
- Desktop layer reviewed 2026-10-31 (SF-013..019). Not yet covered: MFA, CSRF/CORS (bearer-only API), ZAP scan, runtime review on WebView2/WKWebView, updater/signing, AI and business-flow threats.

Phase 1 review: independent security review (OWASP ASVS 5.0.0 lens) of the foundation; findings SF-004..SF-010 fixed the same day.

Baseline scan 2026-10-31 on sample crate: cargo audit, gitleaks, semgrep (p/rust, p/typescript, p/secrets), trivy fs → 0 findings. Not meaningful yet (≈150 lines of code, no dependencies, no server).
