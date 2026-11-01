# Implementation checklist (with dependencies)

Each milestone ends with `scripts/gate.sh full` + independent review + a quality-gate report in `docs/engineering/gates/`.
Legend: ☐ not started · ◐ in progress · ☑ done and verified.

## Phase 0 — Assessment and environment (this session)
- ☑ Environment audit, toolchain install, plugins, agents, hooks, gate script, sample test + scan
- ☑ Architecture proposal, data model, workflow, MVR
- ☑ Founder: "don't wait for my approval", GitHub push allowed (2026-10-31)

## Phase 1 — Foundation (depends on Phase 0 approval)
1. ☑ `crates/server`; env config (`.env.example`, no secrets in repo); `tracing` (JSON via `PROCURE_LOG_FORMAT=json`) with request IDs on every span. `ai`/`storage` crates move to Phase 2/3
2. ☑ Migrations: orgs, users, memberships, roles/permissions, sessions, audit_events (append-only, hash chain), RLS framework — dep 1
3. ☑ Auth: argon2id, login throttling, sessions (create/rotate/revoke/expire), password reset tokens (test-mode delivery), activation/deactivation — dep 2
4. ☑ RBAC permission matrix (7 roles) + `TenantCtx` extractor + RLS `SET LOCAL` per transaction — dep 2,3
5. ☑ Tenant-isolation + permission test suite (real Postgres, per-test schema) — dep 4
6. ☑ OpenAPI (utoipa 5) → `api/openapi.json` → generated TS types (`openapi-typescript`); drift tests on both sides — dep 3
7. ☑ Desktop shell: Tauri 2 app `Chuo-Sogo Procure` (`net.chuo-sogo.procure`), login, in-memory session token (Rust side only), sidebar, role-based nav, customer vs ops workspaces, i18n ja/en/ta/hi + language menu with persistence — dep 6
8. ☑ Desktop tests: Vitest, i18n key coverage, WebdriverIO smoke under Xvfb; Linux `.deb`/AppImage build — dep 7 (Windows/macOS builds configured, not run)

Phase 1 gate: `docs/engineering/gates/phase1.md`.

## Phase 2 — Core workflow (dep Phase 1)
Requests + items + attachments → product identities/evidence/verification → suppliers + approvals → RFQ (test-mode dispatch, export) → quotations → deterministic comparison → recommendation → approval policies (thresholds, multi-step, no self-approval) → audit on every transition.

2a — backend (migration `0002_core_workflow.sql`, `procure_domain::{workflow, approval, comparison}`, `api/{requests,suppliers,rfqs,approvals}.rs`, `tests/phase2.rs`):
1. ☑ Request lifecycle state machine (domain table, side-checked: customer vs operator), PR/RFQ numbering per org/year, audit on every transition
2. ☑ Requests + items: draft editing by the requester only, submit/cancel, clarification round trip (question → answer required), item edits clear verification
3. ☑ Product verification (2c): platform-level product catalogue (`product_identities` + append-only `product_evidence`, https source required to verify); items verified only by linking a verified record; customers see only public sources of records linked to their own items. AI extraction → Phase 3; approved alternatives → later
4. ☑ Supplier directory (platform-level, staff only, internal notes never shown to customers); staff propose a supplier to a customer, the customer approves/rejects (new permission `ApproveSuppliers`)
5. ☑ RFQs to verified suppliers; dispatch = **test channel only** (outbox rows, export endpoint; nothing is sent; supplier text names Chuo-Sogo's RFQ number, never the customer)
6. ☑ Quotations (revisions supersede), deterministic comparison runs stored as snapshots (compliance, reasons, totals in minor units, stable ranking; no FX)
7. ☑ Recommendation: latest comparison only (stale check), compliant quotations only, customer-approved supplier only, reason required if not rank 1
8. ☑ Approval policies per currency (versioned tiers, default 1 approval, no self-approval), snapshot at send; decisions by customer users with ApproveRecommendation only; approve/reject/clarify; append-only approval records
9. ☑ Attachments (2c): content-addressed local storage (0600, hash-verified reads), type allowlist confirmed by content, 10 MB, soft remove, audited; desktop saves to Downloads via a narrow native command (never rendered)
10. ☑ Desktop screens (2b): request list/new/detail (customer edit, submit, cancel, clarification answer), operator intake (pick up, question, item verification with evidence, start sourcing), RFQ create/dispatch (test mode)/outbox, quotation entry, comparison display, recommend/send/withdraw, customer approve/reject/question, supplier directory (staff) and customer supplier decisions, approval policy editor; staff see assigned customer orgs (`/me` → `access: assigned`); ja/en/ta/hi; Vitest + `scripts/e2e-workflow.sh` (full run through the release binary)

## Phase 3 — AI layer (dep Phase 2)
Provider trait + mock + local adapter; schemas per capability; validation + retries + timeouts; `ai_suggestions` with provenance; confidence → review queue; prompt-injection test corpus.

Status (2026-11-01): ☑ `crates/ai` — `Provider` trait, offline deterministic mock (default), OpenAI-compatible adapter (`local` = loopback model server; `remote` needs founder approval flag), strict JSON schemas, `deny_unknown_fields`, retries + timeouts, bounded responses. Capabilities: item extraction from customer text; quotation extraction from supplier text (forced `alternative` on any different part number; exact money). Hallucination guards with flags + confidence caps; `ai_suggestions` with provider/model/prompt-version/hash/latency provenance; requester accepts item suggestions; operator records quotations from a prefilled form (provenance + edited-line count). UI in ja/en/ta/hi. ◐ Real model evaluation pending the founder's provider decision. Product-identity suggestion capability → later.

## Phase 4 — Managed operations (dep Phase 2; parallel with Phase 3)
Task queues, assignment, SLA/overdue, comments vs internal notes, communications log, follow-up jobs, exceptions, service time.

Status (2026-11-01): ☑ migration `0006`; API `ops` — staff work queue across assigned customers (overdue first, orphaned tasks shown as unassigned), tasks (manual + automatic: `identify` on submit with 24 h SLA, `chase_quotes` per RFQ due at the end of the response day in the customer's timezone; closed by the next workflow step, cancelled on cancel/clarification/reject), assignment rules (self vs AssignWork), comments shared vs internal (RLS-enforced), supplier communications log, service time; internal events audited in the platform log. Follow-ups = sent RFQs past their response date with silent suppliers (derived, no background job). UI: Operations queue, comments, staff panel (ja/en/ta/hi).

## Phase 5 — Orders and finance (dep Phase 2)
PO reference, order items, deliveries/receiving, invoice entry (+ extraction via Phase 3), 3-way match, discrepancies, resolution.

Status (2026-11-01): ☑ migration `0007`; `procure_domain::matching` (three-way match, tax per rate with one rounding per rate); API `orders` — customer records its own PO from the approved recommendation (cross-checked), receipts with reversal, supplier invoice entry (lines per order item or off-order, per-line tax rate) matched exactly → reconciled or disputed with itemised discrepancies, resolution with note, close. Orders and Invoices pages; order panel on requests (ja/en/ta/hi). ◐ Invoice AI extraction, partial invoicing and tolerances later.

## Phase 6 — Enterprise readiness (dep 2–5)
Full ja translation review, reports, security hardening pass (ZAP vs local server, ASVS L2 checklist), backup/restore drill, deployment + install/update/troubleshooting docs, CI matrix builds (Win/macOS/Linux), updater, benchmarks.

## Decisions that need the founder
See the reply in chat / `docs/architecture/ARCHITECTURE.md` §8. Summary:
1. Approve this architecture (Rust-first monolith, PostgreSQL, Tauri 2, no Python in v1).
2. App name + bundle ID (proposed: "Chuo-Sogo Procure" / `net.chuo-sogo.procure`).
3. Private GitHub repo creation + push (needed for Win/macOS CI builds).
4. Production hosting, code-signing certificates (Apple $99/yr, Windows OV/EV cert), updater signing-key custody — later, before Phase 6.
5. AI provider for real extraction (local model on this server vs a paid API) — before Phase 3.
6. Real supplier email channel (domain/mailbox) — before turning off test mode.
