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

## Phase 3 — AI layer (dep Phase 2)
Provider trait + mock + local adapter; schemas per capability; validation + retries + timeouts; `ai_suggestions` with provenance; confidence → review queue; prompt-injection test corpus.

## Phase 4 — Managed operations (dep Phase 2; parallel with Phase 3)
Task queues, assignment, SLA/overdue, comments vs internal notes, communications log, follow-up jobs, exceptions, service time.

## Phase 5 — Orders and finance (dep Phase 2)
PO reference, order items, deliveries/receiving, invoice entry (+ extraction via Phase 3), 3-way match, discrepancies, resolution.

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
