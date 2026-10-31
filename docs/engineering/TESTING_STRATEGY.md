# Testing strategy

| Level | Tooling | What |
|---|---|---|
| Unit | cargo test, proptest | `crates/domain`: money, tax, units, state transitions, comparison, approval limits, compliance |
| DB integration | cargo test + real PostgreSQL (isolated database per test run) | migrations, constraints, RLS, repositories, transactions |
| API | cargo test with in-process Axum (`tower::ServiceExt::oneshot`) | auth, permission denials, IDOR, tenant isolation, error format |
| Contract | OpenAPI snapshot + generated TS compile | Rust ↔ TS drift |
| Workflow | API-level scenario tests | request → … → reconciled, per role |
| AI | mock provider; malformed/hostile output corpus | schema rejection, retries, no transactional writes, injection inertness |
| UI | Vitest + Testing Library | components, i18n key coverage, no hardcoded strings (lint rule + test) |
| Desktop E2E | WebdriverIO + @wdio/tauri-service under Xvfb | login, language switch persists across restart, core flow smoke |
| Security | see `docs/security/SECURITY_TEST_PLAN.md` | |
| Performance | criterion benches + `scripts/bench.sh` | comparison/matching at 10k quotes, API p95 |

Rules: tests written before the fix/feature; test count may not drop without a recorded reason; a feature is done only when its acceptance criteria pass end-to-end.
Current state (2026-10-31): 5 unit tests (`procure-domain::money`), all passing.
