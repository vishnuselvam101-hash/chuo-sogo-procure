# Quality gate — Phase 3 (AI layer), 2026-11-01

## Scope
New crate `procure-ai` (providers, capabilities, validation, mock, scripted test provider); `Money::parse_decimal`; migration `0005` (`ai_suggestions`, provenance columns on items and quotations); API `ai` (extract items, extract quotation, get/accept/reject suggestions); AI rate limit (30/h/user) and concurrency bound; desktop: AI item panel on drafts, AI prefill in the quotation form, flags in ja/en/ta/hi.

## Rules enforced
AI output only ever lands in `ai_suggestions`; a requester chooses which item suggestions become items; an operator saves a quotation from a prefilled form. AI never approves, orders or substitutes: a different part number is always an `alternative`. Unknown output fields are rejected; untrusted text is fenced; numbers and part numbers not present in the source are flagged and confidence-capped.

## Automated
`scripts/gate.sh full` — all 15 steps **PASS**.

| Suite | Count | Result |
|---|---|---|
| `procure-domain` unit (incl. exact decimal parsing) | 36 | pass |
| `procure-ai` unit + integration (validation, guards, retries, fencing, injection, mock) | 1 + 10 | pass |
| Server integration Phase 1 / Phase 2–3 (real PostgreSQL) | 18 / 18 | pass |
| Desktop UI (Vitest) | 21 | pass |
| Workflow E2E `scripts/e2e-workflow.sh` (release binaries, mock AI; AI item suggestion ticked by the requester, AI-prefilled quotation saved by the operator) | 24 checks | **24/24 pass** |

Server tests include: injected model output trying to "approve" (rejected, stored as `failed`, no effect), hallucinated MPN (flagged, capped), requester-only acceptance, double-accept refusal, quotation suggestion bound to its supplier/RFQ, per-user rate limit.

## Review
Local independent review: 7 important/low + minor findings → SF-031..SF-035 fixed with regression tests. Residual risks in `docs/security/SECURITY_FINDINGS.md`.

## Not done
No real model evaluated (founder: provider choice). Product-identity suggestion capability, invoice extraction (Phase 5).
