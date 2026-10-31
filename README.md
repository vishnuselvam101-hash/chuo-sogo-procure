# Chuo-Sogo Procure

AI-assisted industrial procurement platform for manufacturers (Japan first). Installable desktop client (Tauri 2) + Rust backend + PostgreSQL.

Status: **Phase 1 (foundation) complete**: authentication, organisations, 7 roles, tenant isolation (PostgreSQL RLS), audit chain, desktop shell (ja/en/ta/hi), Linux installers. Procurement workflow modules start in Phase 2.

Quick start: `docs/engineering/DESKTOP.md` (client), `scripts/db-setup.sh` + `procure-server migrate|bootstrap|serve` (backend).

- Architecture: `docs/architecture/ARCHITECTURE.md`
- Plan: `docs/architecture/IMPLEMENTATION_PLAN.md`
- Engineering: `docs/engineering/` · Security: `docs/security/`
- Quality gate: `scripts/gate.sh quick|full`
