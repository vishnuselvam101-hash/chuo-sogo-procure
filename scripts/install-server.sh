#!/usr/bin/env bash
# Installs (or updates) Chuo-Sogo Procure as a service on this machine.
# Run as root from the repository root after `cargo build --release -p procure-server`.
# Creates: system user `procure`, database `procure_live`, /etc/procure/procure.env (0600,
# generated once, never printed), /var/lib/procure/{storage,outbox,backups}, systemd units.
# Network: the server listens on 127.0.0.1:8089; Tailscale serve publishes it over HTTPS to
# the private tailnet only. Email stays in test mode (files); AI uses the offline mock.
set -euo pipefail
cd "$(dirname "$0")/.."
[ "$(id -u)" = 0 ] || { echo "run as root" >&2; exit 1; }
set -a; . ./.env; set +a
DB=procure_live
OWNER_URL="${DATABASE_URL%/*}/$DB"
RUNTIME_URL="${PROCURE_RUNTIME_DATABASE_URL%/*}/$DB"

id procure >/dev/null 2>&1 || useradd --system --home-dir /var/lib/procure --shell /usr/sbin/nologin procure
install -d -m 0755 /opt/procure/bin
install -d -o procure -g procure -m 0700 /var/lib/procure /var/lib/procure/storage /var/lib/procure/outbox /var/lib/procure/backups /var/lib/procure/work
install -m 0755 target/release/procure-server /opt/procure/bin/procure-server
install -m 0755 scripts/procure-backup.sh /opt/procure/bin/procure-backup
# Off-site copy (B6): installed, but its timer is only enabled once it is configured.
install -m 0755 scripts/procure-offsite.sh /opt/procure/bin/procure-offsite

psql "$DATABASE_URL" -tAc "SELECT 1 FROM pg_database WHERE datname = '$DB'" | grep -q 1 \
  || psql "$DATABASE_URL" -qc "CREATE DATABASE $DB"

install -d -m 0755 /etc/procure
if [ ! -f /etc/procure/procure.env ]; then
  umask 077
  cat > /etc/procure/procure.env <<ENV
DATABASE_URL=$OWNER_URL
PROCURE_RUNTIME_DATABASE_URL=$RUNTIME_URL
PROCURE_BIND=127.0.0.1:8089
PROCURE_STORAGE_DIR=/var/lib/procure/storage
PROCURE_OUTBOX_DIR=/var/lib/procure/outbox
PROCURE_SECRET_KEY=$(openssl rand -hex 32)
PROCURE_TRUST_LOCAL_PROXY=1
PROCURE_ADMIN_ALLOWED_CIDRS=100.64.0.0/10,fd7a:115c:a1e0::/48,127.0.0.1/32,::1/128
PROCURE_AI_PROVIDER=mock
# Invoice PDFs need a Japanese font (apt install fonts-noto-cjk); this is the default.
PROCURE_PDF_FONT=/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc
# Private scratch space for voice notes and catalogue files (deleted after each use).
PROCURE_WORK_DIR=/var/lib/procure/work
# Voice notes for the listing assistant need a local whisper.cpp build and model, e.g.
# PROCURE_STT_COMMAND=/opt/whisper.cpp/build/bin/whisper-cli
# PROCURE_STT_MODEL=/opt/whisper.cpp/models/ggml-small.bin
PROCURE_LOG_FORMAT=json
RUST_LOG=info
ENV
  chmod 0600 /etc/procure/procure.env
fi
# Settings added in later versions, for installs whose env file already exists.
# Backups need a read-only role that bypasses row-level security (forced even for the owner).
if ! grep -q '^PROCURE_BACKUP_DATABASE_URL=' /etc/procure/procure.env; then
  BACKUP_PW="$(openssl rand -hex 24)"
  runuser -u postgres -- env PROCURE_BACKUP_PASSWORD="$BACKUP_PW" psql -v ON_ERROR_STOP=1 -q \
    -v backup_pw="$BACKUP_PW" <<'SQL'
SELECT 'CREATE ROLE procure_backup LOGIN NOSUPERUSER BYPASSRLS NOCREATEDB NOCREATEROLE' WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'procure_backup') \gexec
ALTER ROLE procure_backup PASSWORD :'backup_pw';
GRANT pg_read_all_data TO procure_backup;
SQL
  BACKUP_URL="$(grep '^DATABASE_URL=' /etc/procure/procure.env | cut -d= -f2- | sed -E "s#//[^:]+:[^@]+@#//procure_backup:${BACKUP_PW}@#")"
  echo "PROCURE_BACKUP_DATABASE_URL=$BACKUP_URL" >> /etc/procure/procure.env
  unset BACKUP_PW BACKUP_URL
fi
grep -q '^PROCURE_WORK_DIR=' /etc/procure/procure.env \
  || echo 'PROCURE_WORK_DIR=/var/lib/procure/work' >> /etc/procure/procure.env
# Voice notes: use a local whisper.cpp build when one is installed (scripts/... see README).
if [ -x /opt/whisper.cpp/build/bin/whisper-cli ] && [ -r /opt/whisper.cpp/models/ggml-small.bin ] \
   && ! grep -q '^PROCURE_STT_COMMAND=' /etc/procure/procure.env; then
  printf 'PROCURE_STT_COMMAND=/opt/whisper.cpp/build/bin/whisper-cli\nPROCURE_STT_MODEL=/opt/whisper.cpp/models/ggml-small.bin\n' \
    >> /etc/procure/procure.env
fi

install -m 0644 deploy/procure.service deploy/procure-backup.service deploy/procure-backup.timer \
  deploy/procure-health.service deploy/procure-health.timer deploy/procure-verify.service deploy/procure-verify.timer \
  deploy/procure-offsite.service deploy/procure-offsite.timer \
  /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now procure.service procure-backup.timer procure-health.timer procure-verify.timer
systemctl restart procure.service
for _ in $(seq 40); do curl -fs http://127.0.0.1:8089/health >/dev/null && break; sleep 0.25; done
curl -fs http://127.0.0.1:8089/health
echo
