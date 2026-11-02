#!/usr/bin/env bash
# Phase C desktop E2E: supplier listing (business app) → moderation (admin console) → customer
# catalogue (business app), against a real
# backend on a throwaway database (created and dropped here; dev data is never touched).
# Needs: .env (owner DATABASE_URL, PROCURE_RUNTIME_DATABASE_URL), target/release/procure-server,
# target/release/procure-desktop (business, built with VITE_MARKETPLACE=on: the catalogue is
# off in the factory-side product), target/admin/release/procure-desktop (cargo tauri build --config
# src-tauri/tauri.admin.conf.json --features admin, CARGO_TARGET_DIR=target/admin),
# tauri-driver, WebKitWebDriver, Xvfb, and port 8088 free.
set -euo pipefail
cd "$(dirname "$0")/.."
set -a; . ./.env; set +a
OUT="${E2E_OUT:-reports/e2e-catalogue-$(date -u +%Y%m%dT%H%M%SZ)}"
mkdir -p "$OUT"
WORK="$(mktemp -d)"
DB="procure_e2e_cat_$(date -u +%Y%m%d%H%M%S)"
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

# Speech-to-text stand-in (whisper.cpp is not installed here): real ffmpeg conversion, then a
# script that checks it received a 16 kHz WAV and prints a fixed Japanese narration.
cat > "$WORK/fake-whisper.sh" <<'SH'
#!/bin/sh
head -c 4 "$4" | grep -q RIFF || exit 1
printf 'メーカー: NSK\n型番: 6204-2RS\n品名: 深溝玉軸受 6204-2RS\n状態: 新品\n内径: 20 mm\n'
SH
chmod 700 "$WORK/fake-whisper.sh"
export PROCURE_STT_COMMAND="$WORK/fake-whisper.sh" PROCURE_STT_MODEL="$WORK/model.bin"
# Throwaway key for this run only (encrypts authenticator secrets in the test database).
DATABASE_URL="$OWNER_URL" PROCURE_RUNTIME_DATABASE_URL="$RUNTIME_URL" PROCURE_OUTBOX_DIR="$WORK/outbox" \
  PROCURE_SECRET_KEY="$(openssl rand -hex 32)" PROCURE_BIND=127.0.0.1:8088 \
  target/release/procure-server serve >"$OUT/server.log" 2>&1 &
PIDS+=($!)
for _ in $(seq 40); do curl -fs http://127.0.0.1:8088/health >/dev/null && break; sleep 0.25; done

SEED="$(E2E_API=http://127.0.0.1:8088 E2E_OUTBOX="$WORK/outbox" E2E_ROOT_TOKEN="$ROOT_TOKEN" node apps/desktop/e2e/seed-catalogue.mjs)"

Xvfb :96 -screen 0 1400x900x24 >"$OUT/xvfb.log" 2>&1 &
PIDS+=($!)
sleep 1
DISPLAY=:96 HOME="$WORK" XDG_CONFIG_HOME="$WORK/.config" WEBKIT_DISABLE_DMABUF_RENDERER=1 \
  RUST_LOG=procure_desktop_lib=debug,procure_ui=debug tauri-driver >"$OUT/app-and-driver.log" 2>&1 &
PIDS+=($!)
for _ in $(seq 30); do curl -fs http://127.0.0.1:4444/status >/dev/null && break; sleep 0.5; done

# Real photos (1600×1200 and 1200×1600); metadata removal is covered by the server tests.
mkdir -p "$WORK/photos"
magick -size 1600x1200 gradient:'#1f3a68-#c97a12' -fill white -pointsize 120 -gravity center \
  -annotate 0 '6204-2RS' "$WORK/photos/front.jpg"
magick -size 1200x1600 plasma:fractal -seed 7 "$WORK/photos/side.jpg"
# A 3-second voice note (Opus in Ogg, as phones record) and a catalogue CSV in Shift_JIS.
ffmpeg -nostdin -loglevel error -f lavfi -i "sine=frequency=300:duration=3" -c:a libopus "$WORK/photos/memo.ogg"
printf '外径: 47 mm\n幅: 14 mm\n' | iconv -f UTF-8 -t SHIFT_JIS > "$WORK/photos/catalogue.csv"
E2E_BUSINESS_APP="$PWD/target/release/procure-desktop" E2E_ADMIN_APP="$PWD/target/admin/release/procure-desktop" \
  E2E_OUT="$PWD/$OUT" E2E_SEED="$SEED" E2E_PHOTOS="$WORK/photos" \
  node apps/desktop/e2e/catalogue.mjs | tee "$OUT/result.txt"
