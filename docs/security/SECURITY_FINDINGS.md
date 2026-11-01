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
| SF-020 | 2026-11-01 | Workflow: a recommended/approved quotation could be superseded by a revision; send/decide never re-checked it | Medium | V2 (business logic) | Fixed | quotations frozen after recommendation (`quotations_frozen`), withdraw endpoint, `ensure_still_valid` on send + approve; `review_findings_regressions` |
| SF-021 | 2026-11-01 | RFQs could be sent to suppliers the customer rejected; customer supplier approval checked only at recommendation | Medium | V2 / V8 | Fixed | `supplier_rejected_by_customer` on RFQ creation; supplier approval + verification re-checked on send and approve |
| SF-022 | 2026-11-01 | Customers could read an unsent (internal) recommendation via request detail or recommendation endpoint | Medium | V8 (authorization) | Fixed | customer views require `sent_at IS NOT NULL`; test asserts 404 / null until sent |
| SF-023 | 2026-11-01 | Stale-comparison detection relied on transaction-start timestamps (race window) | Low | V2 | Fixed | comparison stores the exact quotation id set; stale = set differs or run not from today |
| SF-024 | 2026-11-01 | Internal supplier notes protected only by API column lists (row-level policy exposed the column to customer transactions) | Low | V8 | Fixed | notes moved to `supplier_notes` with staff-only RLS; raw-SQL test from customer context sees 0 rows |
| SF-025 | 2026-11-01 | Audit "from" value of supplier status/decision could be pre-wait under concurrent writers | Info | V16 (logging) | Fixed | `SELECT … FOR UPDATE` then `UPDATE` |
| SF-026 | 2026-11-01 | Desktop `api_download` accepted any `/api/v1` path and any file name/extension (a compromised webview could save e.g. `.bat`/`.desktop` files in Downloads) | Medium | V12 (files) | Fixed | command takes org + attachment ids and builds the path itself; only attachment extensions written; never overwrites; partial files removed; 401 clears the token |
| SF-027 | 2026-11-01 | Internal product evidence (notes, supplier confirmations) visible to customers via product detail and the item verification snapshot | Medium | V8 | Fixed | customers get https sources only; snapshot built from public URLs + operator note; regression test |
| SF-028 | 2026-11-01 | A product could be verified on a one-word note | Low | V2 (business logic) | Fixed | verification requires at least one https source |
| SF-029 | 2026-11-01 | Items with attachments could not be deleted (FK error 422) | Low | — | Fixed | attachments detached to the request, audited |
| SF-030 | 2026-11-01 | 30 s timeout too short for 10 MB uploads/downloads; upload decoded before authorization, unbounded concurrency | Low | V13 (resources) | Fixed | uploads 180 s server + client, role check before decode, 4 concurrent uploads |

## Residual risk (Phase 2c)
- Attachment bytes are never deleted (soft-remove keeps them; failed transactions can leave unreferenced files). No sweeper or retention policy yet.
- Content checks are magic-number based: any ZIP passes as .xlsx/.docx, CSV/TXT may contain spreadsheet formulas, PDFs may contain active content. Files are only saved, never opened by the app; the UI warns users.
- One person can create, add evidence to and verify a product record (no four-eyes rule) — founder decision.
- Retiring a product record leaves items already verified against it unchanged (by design; history stays as decided).
- Evidence URLs are not checked for reachability or internal hostnames; they are displayed as text only.

## Residual risk (Phase 2a)
- Zero-price quotation lines are accepted and can rank first (legitimate for samples; operator reviews before recommending).
- A quotation line above the requested quantity (MOQ) is compliant and its total uses the quoted quantity (real cost); not flagged separately yet.
- Self-approval mapped to 403 without a specific code; the UI cannot yet explain why.
- Phase 2a review was static (code reviewer agent) plus the regression tests above; no dynamic testing (ZAP) of the new endpoints yet.

## Residual risk (Phase 1)
- Audit hash chain is unkeyed SHA-256; an attacker with DB owner access could rewrite it. Runtime role cannot. External anchoring (periodic head export / HMAC) planned for Phase 6.
- Rate limits are in-process and keyed on peer IP; behind a reverse proxy they need trusted forwarding headers (not implemented).
- Password-reset timing differs slightly between existing and unknown emails (code paths differ); response is identical.
- Admins cannot cancel a pending invitation yet; the invitee can decline.
- `list_audit` re-verifies the whole chain per request (load concern for large logs).
- Desktop layer reviewed 2026-10-31 (SF-013..019). Not yet covered: MFA, CSRF/CORS (bearer-only API), ZAP scan, runtime review on WebView2/WKWebView, updater/signing, AI and business-flow threats.

Phase 1 review: independent security review (OWASP ASVS 5.0.0 lens) of the foundation; findings SF-004..SF-010 fixed the same day.

Baseline scan 2026-10-31 on sample crate: cargo audit, gitleaks, semgrep (p/rust, p/typescript, p/secrets), trivy fs → 0 findings. Not meaningful yet (≈150 lines of code, no dependencies, no server).
