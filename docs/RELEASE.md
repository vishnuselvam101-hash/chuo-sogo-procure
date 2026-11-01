# Release checklist (1.0)

Nothing on this list is done automatically: tagging, signing, publishing, paid accounts,
store listings and production changes need the founder's explicit approval.

## Ready (verified 2026-11-01)

- [x] `scripts/gate.sh full` — 17 steps PASS (fmt, clippy for both native builds, all Rust and
  UI tests, OpenAPI drift, cargo audit, pnpm audit, Gitleaks tree + history, Semgrep, Trivy)
- [x] End-to-end on release binaries: procurement workflow 30/30, admin console 20/20,
  catalogue (supplier → moderation → customer, voice note + CSV) 13/13, consumer shop
  (sign-up → order → invoice PDF → shipped) + backup + restore drill
- [x] OWASP ZAP authenticated API scan of a throwaway server: 0 fail, 0 warn
- [x] Independent code review at every phase; findings SF-020 … SF-061 fixed with tests
- [x] Backups include stored files; restore drill and `procure-server verify` automated
- [x] Backup role `procure_backup` installed on the production server; first backup made and
  **restore drill passed on the live server** (2026-11-01, backup `20261101T104658Z`)
- [x] Health and integrity monitoring units

## Founder decisions still open

| # | Decision | Needed for |
|---|---|---|
| D1 | Chuo-Sogo's qualified-invoice registration number and bank details (Admin → Billing → Issuer details) | the first real invoice |
| D2 | Bookkeeping/retention rules (電子帳簿保存法) confirmed with the accountant | issuing real invoices |
| D5 | AI model: local model, or a hosted provider (approval: `PROCURE_AI_REMOTE_APPROVED=yes`); vision for nameplate photos | real AI suggestions (the mock only parses "key: value" text) |
| D6 | Consumer shop audience (individuals assumed) and opening date | opening the shop |
| D7 | Card / convenience-store payments (a payment provider is a paid service) | anything beyond bank transfer |
| D8 | Production hosting (this server vs managed hosting in Japan), domain, off-site backups | customers outside Tailscale, phones |
| D9 | Code signing: Windows certificate, Apple Developer ($99/yr) + notarisation, Google Play ($25) | installers without warnings, store listings |

Legal texts in the shop (特定商取引法に基づく表記, returns, privacy policy) must be written or
approved by Chuo-Sogo; the shop cannot open until they are filled in.

## Release steps (after approval)

1. Bump versions (Cargo crates, `tauri*.conf.json`, `package.json`) to `1.0.0`; update
   `CHANGELOG.md`; run `scripts/gate.sh full` and all `scripts/e2e-*.sh`.
2. Server: `scripts/install-server.sh` on the production machine; `scripts/restore-drill.sh`
   on the latest backup; check `systemctl --failed`.
3. Installers: GitHub Actions `desktop-build` (inputs `business`, `admin`, `shop`) and
   `mobile-build`; with signing secrets once D9 is settled (Tauri reads
   `APPLE_CERTIFICATE`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`,
   `APPLE_TEAM_ID`; Windows uses a `signCommand`; Android a release keystore).
4. Tag `v1.0.0` and publish the release notes — founder approval.
5. Distribute: the admin console only to staff machines (never a store); the business app to
   customers; the shop to consumers (stores after D9).
