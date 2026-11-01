# Changelog

## Unreleased — 1.0 release candidate (2026-11-01, not tagged)

- **Phase F — release preparation**: backups now include stored files with checksums;
  `procure-server verify` (audit chains + stored files); restore drill script; health and
  daily integrity-check timers; security headers on every response; OWASP ZAP API scan
  script; consumer data export and account closing; operations runbook and release checklist.
- **Phase E — consumer shop** (Chuo-Sogo Shop app): sign-up, catalogue at tax-inclusive
  prices, cart, orders paid by bank transfer, tax-inclusive qualified invoices issued on
  confirmation, stock reserved on confirmation, payment / shipping / completion,
  cancellation with credit notes, seller notice required before opening; admin Shop section.
- **Phase D — AI listing assistance**: suggestions from typed notes, voice notes (on-server
  speech-to-text) and catalogue files (PDF, Excel, CSV incl. Shift_JIS); translation drafts.
- **Phase C — supplier workspace and catalogue**: supplier organisations, listings with photos
  (metadata removed), moderation, catalogue for business customers.
- **Phase B — invoice generation**: qualified invoices and credit notes, gap-free numbers,
  immutable after issue, PDF with hash in the audit log.
- **Phase A — admin console**: separate staff app, TOTP, network allow-list, organisations,
  accounts, rules, audit, health.
- **Phases 0–5 — procurement platform**: requests, product identification with evidence,
  suppliers, RFQs, quotations, comparison, approvals, orders, receipts, three-way invoice
  matching, operations queue, AI extraction (Phase 3); ja/en/ta/hi.
