# Quality gate — Phase F (release preparation), 2026-11-01

## Done
- **Integrity check** `procure-server verify`: every organisation's audit chain recomputed,
  every stored file (attachments, listing media, invoice PDFs) compared with its SHA-256.
- **Backups** (`scripts/procure-backup.sh`, nightly timer): database + stored files +
  `SHA256SUMS`, 14 kept. **Found and fixed (SF-061):** the previous backup could never have
  worked — row-level security is forced for the table owner, so `pg_dump` as the owner fails.
  Backups now use a dedicated read-only `procure_backup` role with BYPASSRLS (created by
  `install-server.sh` / `scripts/dev-backup-role.sh`); restores run as the PostgreSQL superuser.
- **Restore drill** (`scripts/restore-drill.sh`): checksums → throwaway database → files →
  `verify`; wired into the shop E2E (runs once the backup role exists).
- **Monitoring**: health check every 5 minutes, daily integrity check, service restart on
  failure; admin console Overview.
- **Security**: headers on every response (`nosniff`, `Cross-Origin-Resource-Policy`,
  `no-store`); **OWASP ZAP** authenticated API scan of a throwaway server
  (`scripts/zap-api-scan.sh`): 174 operations, **0 fail / 0 warn / 120 pass** after the fix
  (SF-059). Not a proof of security.
- **Privacy**: consumers can view all data held about them and close their account
  (identity removed, signed out everywhere; orders/invoices kept for tax retention).
- **Documentation**: `docs/OPERATIONS.md`, `docs/RELEASE.md`, `CHANGELOG.md`, README; CI
  installers for all three apps (`desktop-build` input `app`); speech-to-text install script.
- Full gate 17/17 PASS; E2E workflow 30/30, admin 20/20, catalogue 13/13, shop 12/12 on the
  release binaries.

## Production (done by the founder, 2026-11-01)
- `scripts/install-server.sh`: new version, migrations 0009–0012, backup role, monitoring
  timers, speech-to-text (whisper.cpp built by the founder). The first live backup then
  exposed two script bugs (retention step failing when no old dumps exist; the postgres user
  could not read the dump in root's private directory) — both fixed; the **restore drill
  passed on the live server** (backup `20261101T104658Z`, audit chain intact).

## Waiting for the founder
- On the development machine: `scripts/dev-backup-role.sh` (as root), so the shop E2E also
  runs the backup + restore drill (creating the role was denied to automation as a
  permission grant).
- Decisions D1, D2, D5–D9 in `docs/RELEASE.md`; tagging and signing.
