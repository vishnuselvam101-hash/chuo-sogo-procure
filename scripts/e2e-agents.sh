#!/usr/bin/env bash
# Agent layer E2E (L9): the release server on a throwaway database (created and dropped here;
# dev data is never touched), the offline mock AI provider, supplier mail in test mode, and
# scripts/e2e-agents.mjs driving every agent through the HTTP API (the server's own worker
# runs the agents, so a scenario waits up to one 15 s worker tick).
# Needs: .env (owner DATABASE_URL, PROCURE_RUNTIME_DATABASE_URL), target/release/procure-server
# (cargo build --release -p procure-server), node, psql, and port 8088 free.
set -euo pipefail
cd "$(dirname "$0")/.."
set -a; . ./.env; set +a
OUT="${E2E_OUT:-reports/e2e-agents-$(date -u +%Y%m%dT%H%M%SZ)}"
mkdir -p "$OUT"
WORK="$(mktemp -d)"
DB="procure_e2e_agents_$(date -u +%Y%m%d%H%M%S)"
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

# Mock AI (no PROCURE_AI_PROVIDER), no live supplier mail (no PROCURE_SUPPLIER_MAIL).
env -u PROCURE_AI_PROVIDER -u PROCURE_SUPPLIER_MAIL \
  DATABASE_URL="$OWNER_URL" PROCURE_RUNTIME_DATABASE_URL="$RUNTIME_URL" PROCURE_OUTBOX_DIR="$WORK/outbox" \
  PROCURE_STORAGE_DIR="$WORK/storage" PROCURE_WORK_DIR="$WORK/work" PROCURE_BIND=127.0.0.1:8088 \
  target/release/procure-server serve >"$OUT/server.log" 2>&1 &
PIDS+=($!)
for _ in $(seq 40); do curl -fs http://127.0.0.1:8088/health >/dev/null && break; sleep 0.25; done

start=$(date +%s)
E2E_API=http://127.0.0.1:8088 E2E_OUTBOX="$WORK/outbox" E2E_ROOT_TOKEN="$ROOT_TOKEN" E2E_OWNER_URL="$OWNER_URL" \
  node scripts/e2e-agents.mjs | tee "$OUT/result.txt"
echo "elapsed $(( $(date +%s) - start ))s" | tee -a "$OUT/result.txt"
