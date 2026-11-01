# Operations

Everything here runs on Chuo-Sogo's own server. Commands marked **root** change the system and
are run by an operator (the founder), not by automation.

## Install / update (root)

```
cd /root/procurement-platform && git pull && cargo build --release -p procure-server
scripts/install-server.sh
```

`install-server.sh` is idempotent: it creates the `procure` user and directories, writes
`/etc/procure/procure.env` once (random `PROCURE_SECRET_KEY`, never printed), adds settings
introduced by later versions, installs the binary, `procure-backup`, and the systemd units, runs
the database migrations (`ExecStartPre`) and restarts the service. The service listens on
`127.0.0.1:8089`; staff reach it through Tailscale (`tailscale serve --bg --https=443
http://127.0.0.1:8089`).

First start only: `procure-server bootstrap "<company>" <admin-email> "<name>"` (prints a
one-time activation code; use it in the app's "I have a code" and choose a password).

## Settings (`/etc/procure/procure.env`)

| Setting | Meaning |
|---|---|
| `DATABASE_URL` / `PROCURE_RUNTIME_DATABASE_URL` | Owner (migrations, backups, verify) / restricted runtime role |
| `PROCURE_SECRET_KEY` | Encrypts admin authenticator secrets. **Keep a copy offline**: losing it forces every admin to re-enrol |
| `PROCURE_ADMIN_ALLOWED_CIDRS` | Networks allowed to reach the admin API (default: Tailscale + this machine) |
| `PROCURE_STORAGE_DIR` | Attachments, photos, invoice PDFs (content-addressed) |
| `PROCURE_WORK_DIR` | Private scratch space for voice notes and catalogue files |
| `PROCURE_PDF_FONT` | Japanese font for invoice PDFs (`fonts-noto-cjk`) |
| `PROCURE_STT_COMMAND` / `PROCURE_STT_MODEL` | whisper.cpp for voice notes (`scripts/install-whisper.sh`; added automatically when present) |
| `PROCURE_AI_PROVIDER` | `mock` (default). A local model: `local` + base URL; a hosted model additionally needs `PROCURE_AI_REMOTE_APPROVED=yes` (founder decision) |

## Backups and restore

- `procure-backup.timer` 03:30 JST: `pg_dump` + all stored files + `SHA256SUMS` into
  `/var/lib/procure/backups/<time>/`; the newest 14 are kept.
- **These copies are on the same disk.** An off-site copy (another machine or storage
  service) is decision D8 — until then, copy the newest backup directory elsewhere regularly.
- Restore drill (safe, uses a throwaway database):
  `scripts/restore-drill.sh /var/lib/procure/backups/<time> /etc/procure/procure.env`.
  It checks the checksums, restores, and runs `procure-server verify`. Run it monthly and
  after every upgrade.
- Real restore (root, service stopped): `pg_restore --clean --if-exists -d "$DATABASE_URL"
  db.dump`, unpack `storage.tar` into `PROCURE_STORAGE_DIR`, then `procure-server verify`.
  After a restore, check the last issued invoice number against what customers received
  (numbers issued after the backup would be reused otherwise).

## Monitoring

| Unit | When | On failure |
|---|---|---|
| `procure.service` | always | restarts after 5 s |
| `procure-health.timer` | every 5 min | `/health` unreachable → unit failed |
| `procure-verify.timer` | 04:15 JST | an audit chain broken or a stored file missing/changed → unit failed, report in the journal |

Check: `systemctl --failed`, `journalctl -u procure -u procure-verify --since today`, and the
admin console **Overview** (server, database, AI, sessions, locked accounts, per-organisation
warnings and audit-chain status). Notifications to a phone/chat need an outbound channel —
not configured (external communication needs the founder's approval).

## Security checks (on a throwaway local copy, never production)

`scripts/gate.sh full` (Semgrep, Gitleaks, Trivy, cargo/pnpm audit, all tests) and
`scripts/zap-api-scan.sh` (OWASP ZAP authenticated API scan). A clean scan is not proof of
security; see `docs/security/`.
