# Quality gate — Phase 2a (core workflow backend), 2026-11-01

## Scope
Migration `0002_core_workflow.sql`; domain rules `procure_domain::{workflow, approval, comparison}`; API `requests`, `suppliers`, `rfqs`, `approvals` (34 new operations in `api/openapi.json`); integration suite `crates/server/tests/phase2.rs`. Desktop screens for these flows are **not** built yet (Phase 2b).

## Automated
`scripts/gate.sh full` — all 15 steps **PASS** (formatting, API contract drift, typecheck, lint, UI tests, UI build, rustfmt, clippy -D warnings, tests, cargo-audit, pnpm-audit, gitleaks tree + history, semgrep, trivy fs).

| Suite | Count | Result |
|---|---|---|
| `procure-domain` unit (money, RBAC, workflow table, approval policy, comparison) | 34 | pass |
| `procure-server` integration Phase 1 | 18 | pass |
| `procure-server` integration Phase 2 (real PostgreSQL, runtime role) | 10 | pass |
| `procure-server` unit + OpenAPI drift | 4 + 2 | pass |
| Desktop UI (Vitest) | 14 | pass |

Desktop E2E was not re-run: no desktop code changed besides the regenerated API types.

Mutation check: disabling the self-approval rule and the "all items verified" rule each made the suite fail (2 tests), then the code was restored.

Phase 2 integration tests cover: the full request → approval path with a 2-step policy; exact integer totals and ranking; test-mode RFQ messages (recorded, never sent, no customer identity); staff/requester/admin can never approve; self-approval refused; reject/clarify need a comment and clarification needs an answer; recommendation guards (unapproved supplier, non-top rank without reason, non-compliant, stale comparison); workflow guards; draft editing and verification reset; visibility (own requests only for requesters) and tenant isolation (404 across orgs, unassigned staff 404); policy validation and versioning; DB-level cross-tenant FK and RLS checks; regressions for SF-020..SF-024.

## Review
Independent static review of the Phase 2a diff (code-reviewer agent): 3 important and 3 lower findings → SF-020..SF-025, all fixed with regression tests the same day. Residual risks listed in `docs/security/SECURITY_FINDINGS.md`.

## Not done / honest status
- Attachments, full product-identity/evidence tables, AI extraction: not implemented (2b / Phase 3).
- Desktop screens for Phase 2: not implemented.
- No dynamic security scan (ZAP) of the new endpoints yet.
