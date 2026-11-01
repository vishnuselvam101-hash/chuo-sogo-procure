# Quality gate — Phase 2c (attachments, product records), 2026-11-01

## Scope
Migration `0004`; `storage.rs`; API `attachments`, `products`; item verification now links a verified catalogue record; per-router body limits and timeouts; `PayloadTooLarge` (413); desktop native command `api_download`; screens: attachments on requests, product catalogue (staff), verify-item search/create, customer product record view (ja/en/ta/hi).

## Automated
`scripts/gate.sh full` — all 15 steps **PASS**.

| Suite | Count | Result |
|---|---|---|
| Rust domain unit | 36 | pass |
| Server unit (incl. storage, content types, file names, Content-Disposition) | 8 | pass |
| Server integration Phase 1 / Phase 2 (real PostgreSQL, runtime role) | 18 / 14 | pass |
| Desktop native unit (URL/path/method, download names and types, no-overwrite save) | 8 | pass |
| Desktop UI (Vitest) | 20 | pass |
| Workflow E2E `scripts/e2e-workflow.sh` (release binaries, throwaway DB, 3 users) | 20 checks | **20/20 pass** |

E2E now also covers: requester attaches a real PDF through the file picker; operator creates a catalogue record with an https source, verifies it and links the item; customer manager saves the attachment through the native layer into Downloads and the saved bytes match.

Defects found by tests and fixed: oversized bodies without Content-Length reported as 400 instead of 413; Downloads folder missing on Linux without XDG user dirs (fallback `~/Downloads`).

## Review
Local independent review (code-reviewer agent): 0 critical; 5 important/low and several minor → SF-026..SF-030 fixed with regression tests; residual risks in `docs/security/SECURITY_FINDINGS.md`.

## Not done
Attachment retention/sweeper, four-eyes product verification (founder decision), approved alternatives, AI extraction (Phase 3). Native-speaker review of ta/hi strings.
