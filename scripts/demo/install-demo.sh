#!/usr/bin/env bash
# DEMO server for trials (run as root, from the repository, after scripts/install-server.sh):
# a second, separate instance of the same server on 127.0.0.1:8090, filled with fictional
# sample data. It is meant to be public (Tailscale Funnel), so it is kept apart from the real
# system: its own Unix user (procure-demo), database (procure_demo) and database logins
# (procure_demo_owner / procure_demo_app, which cannot connect to procure_live), files
# (/var/lib/procure-demo), secret key and mail outbox; no speech-to-text; a storage quota per
# organisation; no shop self sign-up; trial accounts open from role buttons without a password
# (PROCURE_DEMO_LOGIN). The real system (procure.service) keeps running.
# Publish it with:  tailscale funnel --bg --https=8443 http://127.0.0.1:8090
# Demo accounts are written to /root/procure-demo-accounts.json (owner-only). The trial
# accounts share one password; the platform super-admin has its own.
#   --reset          wipe and re-create the demo data (keeps the shared trial password)
#   --new-password   with --reset: also give the trial accounts a new shared password
# A nightly timer (procure-demo-reset.timer, 03:00 JST) runs --reset so the demo starts each
# day clean, whatever trial users did to it.
set -euo pipefail
umask 077
cd "$(dirname "$0")/../.."
REPO="$PWD"
LIVE_ENV=/etc/procure/procure.env
DEMO_ENV=/etc/procure/demo.env
DATA=/var/lib/procure-demo
ACCOUNTS=/root/procure-demo-accounts.json
RESET=0 NEW_PASSWORD=0
for arg in "$@"; do
  case "$arg" in
    --reset) RESET=1 ;;
    --new-password) NEW_PASSWORD=1 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done
[ -r "$LIVE_ENV" ] || { echo "run scripts/install-server.sh first" >&2; exit 1; }
owner_url="$(grep '^DATABASE_URL=' "$LIVE_ENV" | cut -d= -f2-)"
LIVE_DB="${owner_url##*/}"
DB_HOST="${owner_url#*@}"
DB_HOST="${DB_HOST%%/*}"
pg() { runuser -u postgres -- psql -v ON_ERROR_STOP=1 -qtA "$@"; }

# Installs from before the separation used the live database logins: start those over.
if [ -f "$DEMO_ENV" ] && ! grep -q '^DATABASE_URL=postgres[a-z]*://procure_demo_owner:' "$DEMO_ENV"; then
  echo "demo uses the live database logins; re-creating it with its own" >&2
  RESET=1
fi
password=""
if [ "$RESET" = 1 ]; then
  if [ "$NEW_PASSWORD" = 0 ] && [ -s "$ACCOUNTS" ]; then
    password="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["password"])' "$ACCOUNTS")"
  fi
  systemctl stop procure-demo.service 2>/dev/null || true
  pg -c "DROP DATABASE IF EXISTS procure_demo WITH (FORCE)"
  rm -rf "$DATA" "$DEMO_ENV"
fi

id procure-demo >/dev/null 2>&1 \
  || useradd --system --home-dir "$DATA" --shell /usr/sbin/nologin procure-demo
install -d -o procure-demo -g procure-demo -m 0700 "$DATA" "$DATA/storage" "$DATA/outbox" "$DATA/work"

# Database logins of its own; the live database no longer accepts every role by default.
if [ ! -f "$DEMO_ENV" ]; then
  OWNER_PW="$(openssl rand -hex 24)"
  APP_PW="$(openssl rand -hex 24)"
  pg -v owner_pw="$OWNER_PW" -v app_pw="$APP_PW" <<'SQL'
SELECT 'CREATE ROLE procure_demo_owner LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE' WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'procure_demo_owner') \gexec
SELECT 'CREATE ROLE procure_demo_app LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE' WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'procure_demo_app') \gexec
ALTER ROLE procure_demo_owner PASSWORD :'owner_pw';
ALTER ROLE procure_demo_app PASSWORD :'app_pw';
GRANT procure_runtime TO procure_demo_app;
SQL
  cat > "$DEMO_ENV" <<ENV
DATABASE_URL=postgres://procure_demo_owner:$OWNER_PW@$DB_HOST/procure_demo
PROCURE_RUNTIME_DATABASE_URL=postgres://procure_demo_app:$APP_PW@$DB_HOST/procure_demo
PROCURE_BIND=127.0.0.1:8090
PROCURE_STORAGE_DIR=$DATA/storage
PROCURE_OUTBOX_DIR=$DATA/outbox
PROCURE_WORK_DIR=$DATA/work
PROCURE_SECRET_KEY=$(openssl rand -hex 32)
PROCURE_TRUST_LOCAL_PROXY=1
# The admin console (and super-admin app sign-in) stays private, even when the demo is public.
PROCURE_ADMIN_ALLOWED_CIDRS=100.64.0.0/10,fd7a:115c:a1e0::/48,127.0.0.1/32,::1/128
PROCURE_AI_PROVIDER=mock
PROCURE_ORG_STORAGE_QUOTA_MB=200
PROCURE_SHOP_SIGNUP=off
PROCURE_PDF_FONT=/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc
PROCURE_LOG_FORMAT=json
RUST_LOG=info
ENV
  chmod 0600 "$DEMO_ENV"
  unset OWNER_PW APP_PW
fi
# Trial accounts open from role buttons in the apps, without a password (this server only;
# never the super-admin). Added to existing env files too.
DEMO_LOGIN=ceo@demo-customer.example,manager@demo-customer.example,requester@demo-customer.example,finance@demo-customer.example,operator@demo.chuo-sogo.example,sales@demo-supplier.example,hanako@demo-consumer.example
if grep -q '^PROCURE_DEMO_LOGIN=' "$DEMO_ENV"; then
  sed -i "s#^PROCURE_DEMO_LOGIN=.*#PROCURE_DEMO_LOGIN=$DEMO_LOGIN#" "$DEMO_ENV"
else
  echo "PROCURE_DEMO_LOGIN=$DEMO_LOGIN" >> "$DEMO_ENV"
fi
exists="$(pg -c "SELECT 1 FROM pg_database WHERE datname = 'procure_demo'")"
[ "$exists" = 1 ] || pg -c "CREATE DATABASE procure_demo OWNER procure_demo_owner ENCODING 'UTF8' TEMPLATE template0"
pg -v live="$LIVE_DB" <<'SQL'
REVOKE CONNECT ON DATABASE procure_demo FROM PUBLIC;
GRANT CONNECT ON DATABASE procure_demo TO procure_demo_owner, procure_demo_app;
REVOKE CONNECT ON DATABASE :"live" FROM PUBLIC;
GRANT CONNECT ON DATABASE :"live" TO procure, procure_app;
SELECT format('GRANT CONNECT ON DATABASE %I TO procure_backup', :'live') WHERE EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'procure_backup') \gexec
SQL

install -m 0644 deploy/procure-demo.service /etc/systemd/system/
sed "s#@REPO@#$REPO#g" deploy/procure-demo-reset.service > /etc/systemd/system/procure-demo-reset.service
chmod 0644 /etc/systemd/system/procure-demo-reset.service
install -m 0644 deploy/procure-demo-reset.timer /etc/systemd/system/
systemctl daemon-reload
systemctl enable procure-demo.service procure-demo-reset.timer
systemctl start procure-demo-reset.timer
systemctl restart procure-demo.service  # picks up a changed unit, env file or binary
for _ in $(seq 40); do curl -fs http://127.0.0.1:8090/health >/dev/null && break; sleep 0.5; done
curl -fsS http://127.0.0.1:8090/health
echo

# Sample data, once (an existing demo platform is left as it is).
# demo.env is a systemd EnvironmentFile, not shell code, so it is not sourced here.
demo_owner="$(grep '^DATABASE_URL=' "$DEMO_ENV" | cut -d= -f2-)"
orgs="$(psql "$demo_owner" -tAc "SELECT set_config('app.auth','on',false); SELECT count(*) FROM organisations" | tail -1)"
if [ "$orgs" = 0 ]; then
  token="$(DATABASE_URL="$demo_owner" /opt/procure/bin/procure-server bootstrap "株式会社中央総合（デモ）" admin@demo.chuo-sogo.example "中央総合 管理者（デモ）" | tail -1)"
  photos="$(mktemp -d)"
  partial="$(mktemp /root/.procure-demo-accounts.XXXXXX)"
  trap 'rm -rf "$photos" "$partial"' EXIT
  trap 'echo "demo seed incomplete; run again with --reset" >&2' ERR
  for p in bearing:'#1f3a68-#c97a12':6204-2RS servo:'#2d3436-#74b9ff':HG-KR43 guide:'#00695c-#cfd8dc':HSR15 gloves:'#6a1b9a-#ffd54f':MG-500; do
    IFS=: read -r name grad label <<<"$p"
    magick -size 1600x1200 "gradient:$grad" -fill white -pointsize 140 -gravity center -annotate 0 "$label" "$photos/$name-1.jpg"
    magick -size 1200x1600 "gradient:$grad" -fill white -pointsize 90 -gravity south -annotate +0+120 "DEMO" "$photos/$name-2.jpg"
  done
  [ -n "$password" ] || password="Demo-$(openssl rand -hex 6)"
  # The outbox belongs to procure-demo (0700); the seed reads activation mail from it as root.
  DEMO_API=http://127.0.0.1:8090 DEMO_OUTBOX="$DATA/outbox" DEMO_ROOT_TOKEN="$token" DEMO_PHOTOS="$photos" \
    DEMO_PASSWORD="$password" node scripts/demo/seed-demo.mjs > "$partial"
  chmod 0600 "$partial"
  mv "$partial" "$ACCOUNTS"
  echo "demo data created; accounts in $ACCOUNTS"
elif [ ! -s "$ACCOUNTS" ]; then
  echo "demo data is incomplete (no $ACCOUNTS); run again with --reset" >&2
  exit 1
else
  echo "demo data already present (use --reset to start over)"
fi
