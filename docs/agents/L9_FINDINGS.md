# L9 review findings to fix (2026-11-05)

Security review (S) and code review (C) of a4ffe09^..22f4591. Fix all, each with a regression test.

1. HIGH S1 — chasing.rs:241-245, :362: a *rejected* chase counts as attempt history, so attempt 2+ can auto-send live. Auto-send only if the previous attempt has an outbox row with checked_by NOT NULL (human-approved and sent); a rejected chase stops the series for that PO+type.
2. MEDIUM S2/C1 — exceptions.rs:110-119, :238: decide executes whatever proposed_action is current. DecideRequest carries the `updated_at` (or action hash) the user saw; 409 `exception_changed` on mismatch. UI sends it.
3. MEDIUM S3 — chasing.rs:538, worker.rs:81-83,:127: reply floods drain the budget; failed calls not counted. Rate-limit reply runs per supplier+order (e.g. ≤5/day); count every attempted model call incl. failures.
4. LOW S4 — chasing.rs:617-626: reply document supplier not compared with the order's supplier; manual run with arbitrary ids (api/agents.rs:262). Require equality; validate manual-run inputs belong to org.
5. LOW S5 — api/agents.rs:106-125: CustomerAdmin alone enables auto_send. Make it two-party: customer admin requests, Chuo-Sogo staff confirms (audited); effective only when both set.
6. LOW S6/C5 — queue.rs:244: budget by created_at and UTC month. Count by started_at, month in the org's timezone; supplier_watch use organisations.timezone instead of hard-coded Asia/Tokyo.
7. LOW C2 — integrations.rs ~983: imported invoices force tax_rounding floor and default 10% rate. Make both profile settings; reject rows lacking a rate if the profile has no default.
8. LOW C3 — chasing.rs:417 vs worker.rs:99-106: sweep commits auto-sent outbox rows separately from exceptions/finish. Do both in one transaction.
9. LOW C4 — cost_analyst.rs:221: restore app.staff scope after enable_staff_scope like supplier_watch.rs:689-710.
