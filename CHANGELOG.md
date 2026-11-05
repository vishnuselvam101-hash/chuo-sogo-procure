# Changelog

## 1.1.0 — 2026-11-05

### Procurement agents (migrations 0023–0029, `docs/agents/DESIGN.md`)

Agents propose; a named person approves. AI output is stored only as suggestions, every
applied action is audited, and the AI provider stays the mock until `PROCURE_AI_REMOTE_APPROVED=yes`.

- **Agent runtime**: per-organisation job queue, schedules, monthly AI budget, and an
  exception inbox (approve / edit / reject, version-checked).
- **Matching**: classifies invoice discrepancies with per-organisation tolerances; proposes
  accepting small ones or requesting a credit note.
- **Chasing**: chases suppliers for order confirmations and delivery dates, reads supplier replies
  (prompt-injection checks, per-order rate limit); auto-send only after staff confirmation.
- **Cost analyst**: should-cost and negotiation briefs, negotiation tasks.
- **Supplier watch**: expiring supplier documents and supplier review flags.
- **Import**: CSV/ERP import profiles with tax rounding and default rate settings.
- **Agent console** in the business app and staff confirmation in the admin console.
- Security review (L9) findings fixed with regression tests; end-to-end, injection-corpus
  and worker load tests (`scripts/e2e-agents.sh`).

### Factory-side model (CEO direction 2026-11-02)

- Supplier shortlist for each request (`GET …/requests/{id}/supplier-shortlist`): ranked from the factory's own record and directory categories, about 10 pre-ticked with reasons; the New RFQ form uses it.

See `docs/architecture/FACTORY_SIDE_MODEL.md`.

- **Savings share** instead of a monthly fee: half of the drop from the price the factory
  paid before, for 36 months, ¥0 when the price did not go down. Baselines fixed per item
  from the factory's own pre-contract records (or agreed and confirmed by the factory);
  immutable ledger written when an order reconciles; fee invoice drafts from the admin
  console; Savings page for the factory (migration 0016).
- **Staff checks**: an RFQ is checked by our staff before it is sent, and a comparison
  before the factory sees the recommendation (migration 0017).
- **Supplier replies from files**: saved emails (.eml, Japanese charsets, attachments),
  PDF, Excel/CSV and FAX scans read into text for AI extraction.
- **Delivery dates**: promised date from the quoted lead time, the supplier's confirmed
  date, an automatic check-delivery task and a Late flag (migration 0018).
- The customer catalogue is off unless built with `VITE_MARKETPLACE=on`: we are not a
  marketplace.

## 1.0.0 — 2026-11-01 (installers built; not tagged, unsigned)

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
