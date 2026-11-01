#!/usr/bin/env bash
# Phase E desktop E2E: the consumer shop app (release build with tauri.shop.conf.json) against a real
# backend on a throwaway database (created and dropped here; dev data is never touched).
# Needs: .env (owner DATABASE_URL, PROCURE_RUNTIME_DATABASE_URL), target/release/procure-server,
# target/shop/release/procure-desktop (cargo tauri build --config src-tauri/tauri.shop.conf.json,
# CARGO_TARGET_DIR=target/shop), ImageMagick,
# tauri-driver, WebKitWebDriver, Xvfb, and port 8088 free.
set -euo pipefail
cd "$(dirname "$0")/.."
set -a; . ./.env; set +a
OUT="${E2E_OUT:-reports/e2e-shop-$(date -u +%Y%m%dT%H%M%SZ)}"
mkdir -p "$OUT"
WORK="$(mktemp -d)"
DB="procure_e2e_shop_$(date -u +%Y%m%d%H%M%S)"
OWNER_URL="${DATABASE_URL%/*}/$DB"
RUNTIME_URL="${PROCURE_RUNTIME_DATABASE_URL%/*}/$DB"
PIDS=()
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done
  sleep 1
  psql "$DATABASE_URL" -qc "DROP DATABASE IF EXISTS $DB WITH (FORCE)" >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

if curl -fs http://127.0.0.1:8088/health >/dev/null; then
  echo "port 8088 is in use; stop the dev server first" >&2
  exit 1
fi
psql "$DATABASE_URL" -qc "CREATE DATABASE $DB"
ROOT_TOKEN="$(DATABASE_URL="$OWNER_URL" target/release/procure-server bootstrap "Chuo-Sogo Inc." root@chuo-sogo.e2e "Root Admin" | tail -1)"

# Throwaway key for this run only (encrypts authenticator secrets in the test database).
DATABASE_URL="$OWNER_URL" PROCURE_RUNTIME_DATABASE_URL="$RUNTIME_URL" PROCURE_OUTBOX_DIR="$WORK/outbox" \
  PROCURE_SECRET_KEY="$(openssl rand -hex 32)" PROCURE_BIND=127.0.0.1:8088 \
  target/release/procure-server serve >"$OUT/server.log" 2>&1 &
PIDS+=($!)
for _ in $(seq 40); do curl -fs http://127.0.0.1:8088/health >/dev/null && break; sleep 0.25; done

mkdir -p "$WORK/photos"
magick -size 1600x1200 gradient:'#1f3a68-#c97a12' "$WORK/photos/front.jpg"
magick -size 1200x1600 plasma:fractal -seed 7 "$WORK/photos/side.jpg"
SEED="$(E2E_API=http://127.0.0.1:8088 E2E_OUTBOX="$WORK/outbox" E2E_ROOT_TOKEN="$ROOT_TOKEN" E2E_PHOTOS="$WORK/photos" node apps/desktop/e2e/seed-shop.mjs)"
# The shop app starts in English against this server (its own preferences file).
mkdir -p "$WORK/.config/net.chuo-sogo.shop"
printf '{"language":"en","server_url":"http://127.0.0.1:8088"}' > "$WORK/.config/net.chuo-sogo.shop/prefs.json"

Xvfb :95 -screen 0 1400x900x24 >"$OUT/xvfb.log" 2>&1 &
PIDS+=($!)
sleep 1
DISPLAY=:95 HOME="$WORK" XDG_CONFIG_HOME="$WORK/.config" WEBKIT_DISABLE_DMABUF_RENDERER=1 \
  RUST_LOG=procure_desktop_lib=debug,procure_ui=debug tauri-driver >"$OUT/app-and-driver.log" 2>&1 &
PIDS+=($!)
for _ in $(seq 30); do curl -fs http://127.0.0.1:4444/status >/dev/null && break; sleep 0.5; done

E2E_APP="$PWD/target/shop/release/procure-desktop" E2E_OUT="$PWD/$OUT" E2E_SEED="$SEED" E2E_HOME="$WORK" \
  E2E_API=http://127.0.0.1:8088 E2E_OUTBOX="$WORK/outbox" node apps/desktop/e2e/shop.mjs | tee "$OUT/result.txt"

# Operations check on the same data: nightly backup, then a full restore drill (checksums,
# restore into a throwaway database, audit chains and every stored file verified). Needs the
# procure_backup role (scripts/dev-backup-role.sh, as root) and root for the restore.
if [ -z "${PROCURE_BACKUP_DATABASE_URL:-}" ]; then
  echo "SKIPPED backup + restore drill: no procure_backup role (run scripts/dev-backup-role.sh as root)" \
    | tee -a "$OUT/result.txt"
  exit 0
fi
BACKUP_URL="$PROCURE_BACKUP_DATABASE_URL"
PROCURE_BACKUP_DIR="$WORK/backups" PROCURE_BACKUP_DATABASE_URL="${BACKUP_URL%/*}/$DB" PROCURE_STORAGE_DIR="$PWD/var/storage" \
  scripts/procure-backup.sh | tee -a "$OUT/result.txt"
printf 'DATABASE_URL=%s\n' "$OWNER_URL" > "$WORK/drill.env"
scripts/restore-drill.sh "$(ls -d "$WORK"/backups/2*/ | head -1)" "$WORK/drill.env" 2>&1 | tail -12 | tee -a "$OUT/result.txt"
