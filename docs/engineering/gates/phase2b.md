# Quality gate — Phase 2b (desktop workflow screens), 2026-11-01

## Scope
Desktop screens for the Phase 2 workflow: requests (list, new, detail), operator intake and item verification, RFQ creation/test-mode dispatch/outbox, quotation entry, comparison display, recommend/send/withdraw, customer approve/reject/question, supplier directory (staff) and customer supplier decisions, approval policy editor. Backend additions: `/me` lists the customer orgs a staff member is assigned to (`access: "assigned"`, migration `0003`), `latest_comparison_id` on RFQ detail. Desktop bridge allows `PUT`.

## Automated
`scripts/gate.sh full` — all 15 steps **PASS**.

| Suite | Count | Result |
|---|---|---|
| Rust domain unit | 34 | pass |
| Server integration (Phase 1 / Phase 2) | 18 / 11 | pass |
| Desktop native unit | 5 | pass |
| Desktop UI (Vitest): shell, i18n parity (ja/en/ta/hi), money parsing, error details, operator verification, approval controls staff vs customer, new request validation | 19 | pass |
| **Workflow E2E** `scripts/e2e-workflow.sh` (release binaries, throwaway DB, three users) | 17 checks | **17/17 pass** |

The E2E drives one request end to end through the real app: requester drafts and submits; the assigned operator picks up, verifies the item with evidence, starts sourcing, creates and dispatches an RFQ (test mode, 2 messages, customer name absent), records two quotations, runs the comparison (exact ¥120,000 total, rank 1), recommends and sends; the operator is never offered Approve; the customer manager sees no internal supplier notes and approves; the request ends `approved`. Screenshots in `reports/e2e-workflow-*/` (local, not committed).

Defects found by the E2E and fixed: missing layout styles let an overflowing table block the quotation Save button; test-script supplier ordering (test bug, not app). The Phase 1 smoke E2E (`scripts/e2e-desktop.sh`) was not re-run in this increment.

## Not done
Attachments, product identity/evidence tables, AI extraction; Windows/macOS builds; no live security scan (ZAP) yet. Tamil and Hindi strings were written for this increment and have not been reviewed by a native speaker.
