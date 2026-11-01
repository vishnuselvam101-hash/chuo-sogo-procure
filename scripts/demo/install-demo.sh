#!/usr/bin/env bash
# DEMO server for trials (run as root, from the repository, after scripts/install-server.sh):
# a second, separate instance of the same server on 127.0.0.1:8090 with its own database
# (procure_demo), files (/var/lib/procure-demo) and secret key, filled with fictional sample
# data. The real system (procure.service, procure_live) is not touched.
# Publish it with:  tailscale funnel --bg --https=8443 http://127.0.0.1:8090
# Demo accounts are written to /root/procure-demo-accounts.txt (owner-only).
# Re-run with --reset to wipe and re-create the demo data.
set -euo pipefail
cd "$(dirname "$0")/../.."
LIVE_ENV=/etc/procure/procure.env
DEMO_ENV=/etc/procure/demo.env
DATA=/var/lib/procure-demo
[ -r "$LIVE_ENV" ] || { echo "run scripts/install-server.sh first" >&2; exit 1; }
owner_url="$(grep '^DATABASE_URL=' "$LIVE_ENV" | cut -d= -f2-)"
runtime_url="$(grep '^PROCURE_RUNTIME_DATABASE_URL=' "$LIVE_ENV" | cut -d= -f2-)"
DEMO_OWNER="${owner_url%/*}/procure_demo"
DEMO_RUNTIME="${runtime_url%/*}/procure_demo"

if [ "${1:-}" = "--reset" ]; then
  systemctl stop procure-demo.service 2>/dev/null || true
  psql "$owner_url" -qc "DROP DATABASE IF EXISTS procure_demo WITH (FORCE)"
  rm -rf "$DATA"
fi

id procure >/dev/null
install -d -o procure -g procure -m 0700 "$DATA" "$DATA/storage" "$DATA/outbox" "$DATA/work"
psql "$owner_url" -tAc "SELECT 1 FROM pg_database WHERE datname = 'procure_demo'" | grep -q 1 \
  || psql "$owner_url" -qc "CREATE DATABASE procure_demo"
if [ ! -f "$DEMO_ENV" ]; then
  umask 077
  cat > "$DEMO_ENV" <<ENV
DATABASE_URL=$DEMO_OWNER
PROCURE_RUNTIME_DATABASE_URL=$DEMO_RUNTIME
PROCURE_BIND=127.0.0.1:8090
PROCURE_STORAGE_DIR=$DATA/storage
PROCURE_OUTBOX_DIR=$DATA/outbox
PROCURE_WORK_DIR=$DATA/work
PROCURE_SECRET_KEY=$(openssl rand -hex 32)
PROCURE_TRUST_LOCAL_PROXY=1
# The admin console stays private (Tailscale and this machine), even when the demo is public.
PROCURE_ADMIN_ALLOWED_CIDRS=100.64.0.0/10,fd7a:115c:a1e0::/48,127.0.0.1/32,::1/128
PROCURE_AI_PROVIDER=mock
PROCURE_PDF_FONT=/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc
PROCURE_LOG_FORMAT=json
RUST_LOG=info
ENV
  grep -E '^PROCURE_STT_(COMMAND|MODEL)=' "$LIVE_ENV" >> "$DEMO_ENV" || true
  chmod 0600 "$DEMO_ENV"
fi
install -m 0644 deploy/procure-demo.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now procure-demo.service
for _ in $(seq 40); do curl -fs http://127.0.0.1:8090/health >/dev/null && break; sleep 0.5; done
curl -fsS http://127.0.0.1:8090/health

# Sample data, once (an existing demo platform is left as it is).
set -a; . "$DEMO_ENV"; set +a
if psql "$DATABASE_URL" -tAc "SELECT set_config('app.auth','on',false); SELECT count(*) FROM organisations" | tail -1 | grep -qx 0; then
  token="$(/opt/procure/bin/procure-server bootstrap "株式会社中央総合（デモ）" admin@demo.chuo-sogo.example "中央総合 管理者（デモ）" | tail -1)"
  photos="$(mktemp -d)"
  for p in bearing:'#1f3a68-#c97a12':6204-2RS servo:'#2d3436-#74b9ff':HG-KR43 guide:'#00695c-#cfd8dc':HSR15 gloves:'#6a1b9a-#ffd54f':MG-500; do
    IFS=: read -r name grad label <<<"$p"
    magick -size 1600x1200 "gradient:$grad" -fill white -pointsize 140 -gravity center -annotate 0 "$label" "$photos/$name-1.jpg"
    magick -size 1200x1600 "gradient:$grad" -fill white -pointsize 90 -gravity south -annotate +0+120 "DEMO" "$photos/$name-2.jpg"
  done
  password="Demo-$(openssl rand -hex 6)"
  DEMO_API=http://127.0.0.1:8090 DEMO_OUTBOX="$DATA/outbox" DEMO_ROOT_TOKEN="$token" DEMO_PHOTOS="$photos" \
    DEMO_PASSWORD="$password" node scripts/demo/seed-demo.mjs > /root/procure-demo-accounts.json
  chmod 0600 /root/procure-demo-accounts.json
  rm -rf "$photos"
  echo "demo data created; accounts in /root/procure-demo-accounts.json"
else
  echo "demo data already present (use --reset to start over)"
fi
