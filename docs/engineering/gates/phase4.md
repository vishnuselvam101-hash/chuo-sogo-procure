# Quality gate — Phase 4 (managed operations), 2026-11-01

## Scope
Migration `0006` (tasks, comments, communications, time_entries; RLS with staff scope); API `ops` (queue, tasks, comments, communications, time); automatic tasks hooked into submit / start sourcing / RFQ dispatch / comparison and cancelled when a request leaves Chuo-Sogo's hands; internal events audited in the platform (Chuo-Sogo) log, never the customer's; desktop: Operations queue, comments (shared/internal), staff panel.

## Automated
`scripts/gate.sh full` — all 15 steps **PASS**.

| Suite | Count | Result |
|---|---|---|
| Server integration Phase 2–4 (real PostgreSQL) | 23 | pass |
| Desktop UI (Vitest) | 21 | pass |
| Workflow E2E (release binaries): operator starts from the Operations queue (automatic identify task), posts an internal note and a shared comment; the customer sees the shared one only | 26 checks | **26/26 pass** |

Key tests: internal comments invisible to customers through the API and at the database level (customer-context transaction counts 0 internal rows, 0 tasks/communications/time); customer audit log never contains comment text or internal events; queue scoped to assigned customers and to roles that may operate; per-RFQ chase tasks with timezone-correct due dates; cancellation/clarification stop automatic tasks; assignment rules.

## Review
Local independent review: no customer-facing leak; 5 medium/low findings + UI points → SF-036..SF-038 fixed with regression tests.

## Not done
Per-customer SLA settings; reminders/notifications (external messaging needs approval); clearing due dates.
