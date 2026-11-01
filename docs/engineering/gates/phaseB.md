# Quality gate — Phase B (invoice generation), 2026-11-01

## Scope
Invoices Chuo-Sogo issues to its customers (Japanese qualified invoices, 適格請求書) and
credit notes (適格返還請求書):
- `procure_domain::invoicing`: registration number check (T + 13 digits, corporate-number
  check digit), totals per tax rate (10 % / 8 % reduced / 0 %) with tax rounded once per rate
  per invoice, credit limits per rate for base **and** tax.
- Migration `0009`: issuer profile, issued invoices + lines, gap-free counters per series and
  year; triggers make issued documents immutable (only the payment status can change); RLS:
  customers see only their own issued documents, only platform-scoped (admin) transactions
  see drafts or write.
- Server PDF (`invoice_pdf`, krilla + Noto Sans CJK): bilingual labels, ※ for reduced-rate
  lines, per-rate summary, registration number, bank details, multi-page; deterministic;
  stored content-addressed, SHA-256 in the audit log. Text that does not fit is refused
  (never printed cut off).
- API: admin (`manage_billing`) issuer profile, drafts, issue, payment / undo, credit notes,
  PDF; customer (`view_finance`, customer members) list, detail, PDF.
- Desktop: admin console **Billing** (list with filters, editor, issue with confirmation, PDF
  download, payment, credit notes, issuer details); business app **Chuo-Sogo invoices**
  page for finance. Native `invoice_download` saves `<number>.pdf` to Downloads (both apps).
  ja/en/ta/hi.

## Automated
`scripts/gate.sh full` — all 17 steps **PASS**.

| Suite | Count | Result |
|---|---|---|
| Domain invoicing (rounding per rate, registration numbers, credit limits incl. tax cap) | 6 | pass |
| PDF render (1 and 4 pages, determinism, refusal of over-long text) | 2 | pass |
| Server billing integration (real PostgreSQL; immutability also tested directly against the database as table owner) | 5 | pass |
| Desktop UI (Vitest) | 33 | pass |
| Admin E2E (release binaries): … issuer → draft (server computes ¥1,324) → issue CS-YYYY-00001 → PDF saved to Downloads → paid → credit note CS-CN-YYYY-00001 | 20 checks | **20/20 pass** |
| Business workflow E2E (regression) | 30 checks | **30/30 pass** |

## Review
Local independent review: no tenant isolation, numbering or concurrency holes. Fixed with
regression tests: SF-044 (credit notes could return more tax than charged under ceil/half-up
rounding), SF-045 (over-long notes would have been cut off on an immutable PDF), SF-046
(editing a draft reset a custom recipient name; issuer snapshot and document rounding could
disagree; paying twice overwrote the payment record; issue dates could go backwards behind
the number sequence).

## Not done / decisions
- [D1] Chuo-Sogo's real registration number must be entered in Issuer details before the
  first real invoice (the test uses the National Tax Agency's public number).
- [D2] Retention/bookkeeping (電子帳簿保存法) still to be confirmed with the accountant.
- No e-invoice exchange (Peppol / JP PINT), no automatic payment matching, no emailing of
  invoices (download and send manually).
