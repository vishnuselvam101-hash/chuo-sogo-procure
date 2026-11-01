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
install -d -o procure -g procure -m 0700 /var/lib/procure /var/lib/procure/storage /var/lib/procure/outbox /var/lib/procure/backups
install -m 0755 target/release/procure-server /opt/procure/bin/procure-server

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
PROCURE_LOG_FORMAT=json
RUST_LOG=info
ENV
  chmod 0600 /etc/procure/procure.env
fi

install -m 0644 deploy/procure.service deploy/procure-backup.service deploy/procure-backup.timer /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now procure.service procure-backup.timer
systemctl restart procure.service
for _ in $(seq 40); do curl -fs http://127.0.0.1:8089/health >/dev/null && break; sleep 0.25; done
curl -fs http://127.0.0.1:8089/health
echo
