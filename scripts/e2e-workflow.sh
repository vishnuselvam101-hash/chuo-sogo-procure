#!/usr/bin/env bash
# Phase 2 desktop E2E: a full request -> approval run through the real release binary and a
# real backend on a throwaway database (created and dropped here; dev data is never touched).
# Needs: .env (owner DATABASE_URL, PROCURE_RUNTIME_DATABASE_URL), a release build of both
# binaries, tauri-driver, WebKitWebDriver, Xvfb, and port 8088 free.
set -euo pipefail
cd "$(dirname "$0")/.."
set -a; . ./.env; set +a
OUT="${E2E_OUT:-reports/e2e-workflow-$(date -u +%Y%m%dT%H%M%SZ)}"
mkdir -p "$OUT"
WORK="$(mktemp -d)"
DB="procure_e2e_$(date -u +%Y%m%d%H%M%S)"
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

DATABASE_URL="$OWNER_URL" PROCURE_RUNTIME_DATABASE_URL="$RUNTIME_URL" PROCURE_OUTBOX_DIR="$WORK/outbox" \
  PROCURE_BIND=127.0.0.1:8088 target/release/procure-server serve >"$OUT/server.log" 2>&1 &
PIDS+=($!)
for _ in $(seq 40); do curl -fs http://127.0.0.1:8088/health >/dev/null && break; sleep 0.25; done

SEED="$(E2E_API=http://127.0.0.1:8088 E2E_OUTBOX="$WORK/outbox" E2E_ROOT_TOKEN="$ROOT_TOKEN" node apps/desktop/e2e/seed.mjs)"

Xvfb :98 -screen 0 1400x900x24 >"$OUT/xvfb.log" 2>&1 &
PIDS+=($!)
sleep 1
DISPLAY=:98 HOME="$WORK" XDG_CONFIG_HOME="$WORK/.config" WEBKIT_DISABLE_DMABUF_RENDERER=1 \
  RUST_LOG=procure_desktop_lib=debug,procure_ui=debug tauri-driver >"$OUT/app-and-driver.log" 2>&1 &
PIDS+=($!)
for _ in $(seq 30); do curl -fs http://127.0.0.1:4444/status >/dev/null && break; sleep 0.5; done

E2E_APP="$PWD/target/release/procure-desktop" E2E_OUT="$PWD/$OUT" E2E_SEED="$SEED" \
  node apps/desktop/e2e/workflow.mjs | tee "$OUT/result.txt"
