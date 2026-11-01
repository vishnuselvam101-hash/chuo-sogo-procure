# Quality gate — Phase 5 (orders and finance), 2026-11-01

## Scope
`procure_domain::matching` (three-way match; tax per rate, rounded once per rate: floor / half-up / ceil); migration `0007`; API `orders` (record PO, receipts and reversal, invoice entry and matching, discrepancy resolution, close); request list stages (`orders`, `finance`); desktop: order panel, Orders and Invoices pages (ja/en/ta/hi). New permissions: RecordOrders (customer admin, procurement manager), ReceiveGoods (also requesters, own requests). Staff never record orders or invoices.

## Automated
`scripts/gate.sh full` — all 15 steps **PASS**.

| Suite | Count | Result |
|---|---|---|
| `procure-domain` unit (matching, tax rounding, workflow, RBAC…) | 43 | pass |
| Server integration Phase 2–5 (real PostgreSQL) | 26 | pass |
| Desktop UI (Vitest) | 21 | pass |
| Workflow E2E (release binaries): request → AI items → verification → RFQ → AI-prefilled quotation → comparison → approval → customer PO → receipt → finance invoice typed from the supplier's figures → matched → reconciled → closed | 30 checks | **30/30 pass** |

## Review
Local independent review: no cross-tenant or authorization holes; 1 high + 6 medium/low → SF-039..SF-041 fixed with regression tests (under-invoicing, receipt reversal, off-order lines, mixed tax rates, duplicate supplier invoice numbers, PO cross-check, invoice form hints).

## Not done
Partial/multiple invoices per order, tolerances, invoice AI extraction, over-delivery booking.
