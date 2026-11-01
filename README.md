# Chuo-Sogo Procure

AI-assisted industrial procurement platform (Japan first) by Chuo-Sogo Inc. One Rust server
and PostgreSQL database (row-level security for every organisation), three desktop/mobile apps
built with Tauri 2:

| App | For | Build |
|---|---|---|
| **Chuo-Sogo Procure** | Business customers (requests, approvals, orders, invoices), suppliers (product listings), Chuo-Sogo operators | `cargo tauri build` |
| **Chuo-Sogo Procure Admin** | Chuo-Sogo staff only: organisations, accounts, rules, billing, listing review, shop, audit, health (TOTP sign-in, network allow-list) | `cargo tauri build --config src-tauri/tauri.admin.conf.json --features admin` |
| **Chuo-Sogo Shop** | Individual consumers: catalogue, cart, orders paid by bank transfer, invoices | `cargo tauri build --config src-tauri/tauri.shop.conf.json` |

Status (2026-11-01): Phases 0–5 (procurement), A (admin console), B (invoice generation),
C (supplier catalogue), D (AI listing assistance), E (consumer shop) and F (release
preparation) implemented and tested — see `docs/engineering/gates/` and `CHANGELOG.md`.
Not yet released: signing, store listings and production decisions are listed in
`docs/RELEASE.md`.

- Operations (install, update, backups, restore drill, monitoring): `docs/OPERATIONS.md`
- Release checklist and open founder decisions: `docs/RELEASE.md`
- Architecture: `docs/architecture/` · Engineering: `docs/engineering/` · Security: `docs/security/`
- Quality gate: `scripts/gate.sh quick|full` · End-to-end: `scripts/e2e-{workflow,admin,catalogue,shop}.sh`
- Security scan of a throwaway local server: `scripts/zap-api-scan.sh`
