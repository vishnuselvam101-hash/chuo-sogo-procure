# Chuo-Sogo Procure — project rules

AI industrial procurement platform: we do the factory's buying work for the factory (not a marketplace), paid by a share of the savings (`docs/architecture/FACTORY_SIDE_MODEL.md`). Japan-first desktop app. Owner: Chuo-Sogo Inc.
Read `docs/architecture/ARCHITECTURE.md` and `docs/architecture/IMPLEMENTATION_PLAN.md` before changing anything.

## Non-negotiables
- Rust is authoritative for business, money, permissions, workflow. TypeScript is presentation only.
- Every customer-owned query is tenant-scoped (`TenantCtx`) and protected by PostgreSQL RLS.
- Money: `procure_domain::money::Money` (integer minor units). No floats, no implicit FX.
- AI output goes to `ai_suggestions` only; a human promotes it. AI never approves, never issues POs, never substitutes products.
- Every workflow transition emits an audit event in the same transaction.
- No hardcoded UI strings; use `apps/desktop/locales/<lang>/`.
- Supplier email is test mode unless explicitly configured and approved.

## Commands
- `scripts/gate.sh quick` during work; `scripts/gate.sh full` at milestones.
- Rust 1.95 (`rust-toolchain.toml`). `cargo tauri`, `sqlx`, `cargo audit` installed.

## Process
Agents in `.claude/agents/`, used sparingly: implement in the main session; call code-reviewer, cybersecurity-expert, debugger, qa-engineer at milestone gates.
Commits: author vishnuselvam101-hash, no AI attribution trailers. Push to the private GitHub repo is allowed (founder, 2026-10-31). Never tag a release, deploy, sign, buy services, or email suppliers/customers without founder approval.

## Database
`scripts/db-setup.sh` (once, as postgres): owner role `procure` runs migrations (`DATABASE_URL`); the server runs as `procure_app` ∈ `procure_runtime` (`PROCURE_RUNTIME_DATABASE_URL`), DML only. Tests use `#[sqlx::test]` databases and `SET ROLE procure_runtime`.
