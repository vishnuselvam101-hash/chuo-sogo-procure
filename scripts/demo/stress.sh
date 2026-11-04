#!/usr/bin/env bash
# Heavy load test (scripts/demo/stress.mjs) against a release backend on a throwaway database
# (created and dropped here; live and demo data are never touched). Samples the server's memory
# every 10 s and checks it is still healthy at the end.
#   scripts/demo/stress.sh [minutes] [virtual users]
# Needs: .env (owner DATABASE_URL, PROCURE_RUNTIME_DATABASE_URL), target/release/procure-server,
# port 8097 free.
set -euo pipefail
cd "$(dirname "$0")/../.."
set -a; . ./.env; set +a
MIN="${1:-10}" USERS="${2:-96}"
OUT="${STRESS_DIR:-reports/stress-$(date -u +%Y%m%dT%H%M%SZ)}"
mkdir -p "$OUT"
WORK="$(mktemp -d)"
DB="procure_stress_$(date -u +%Y%m%d%H%M%S)"
OWNER_URL="${DATABASE_URL%/*}/$DB"
RUNTIME_URL="${PROCURE_RUNTIME_DATABASE_URL%/*}/$DB"
PID=""
cleanup() {
  [ -n "$PID" ] && kill "$PID" 2>/dev/null || true
  sleep 1
  psql "$DATABASE_URL" -qc "DROP DATABASE IF EXISTS $DB WITH (FORCE)" >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

if curl -fs http://127.0.0.1:8097/health >/dev/null; then echo "port 8097 is in use" >&2; exit 1; fi
psql "$DATABASE_URL" -qc "CREATE DATABASE $DB"
DATABASE_URL="$OWNER_URL" target/release/procure-server migrate >"$OUT/migrate.log" 2>&1
ROOT_TOKEN="$(DATABASE_URL="$OWNER_URL" target/release/procure-server bootstrap "Chuo-Sogo Inc." root@chuo-sogo.stress "Root Admin" | tail -1)"
DATABASE_URL="$OWNER_URL" PROCURE_RUNTIME_DATABASE_URL="$RUNTIME_URL" PROCURE_OUTBOX_DIR="$WORK/outbox" \
  PROCURE_STORAGE_DIR="$WORK/storage" PROCURE_BIND=127.0.0.1:8097 PROCURE_LOG_FORMAT=json \
  target/release/procure-server serve >"$OUT/server.log" 2>&1 &
PID=$!
for _ in $(seq 40); do curl -fs http://127.0.0.1:8097/health >/dev/null && break; sleep 0.25; done

( echo "seconds,rss_kb,threads"; s=0
  while kill -0 "$PID" 2>/dev/null; do
    echo "$s,$(awk '/VmRSS/{print $2}' /proc/$PID/status),$(awk '/Threads/{print $2}' /proc/$PID/status)"
    sleep 10; s=$((s + 10))
  done ) >"$OUT/memory.csv" &
MON=$!

set +e
STRESS_API=http://127.0.0.1:8097 STRESS_OUTBOX="$WORK/outbox" STRESS_ROOT_TOKEN="$ROOT_TOKEN" \
  STRESS_MINUTES="$MIN" STRESS_USERS="$USERS" STRESS_OUT="$OUT/result.json" \
  node scripts/demo/stress.mjs 2>"$OUT/stress.log" | tail -1
RC=${PIPESTATUS[0]}
set -e
kill "$MON" 2>/dev/null || true
if ! kill -0 "$PID" 2>/dev/null; then echo "server died during the test" >&2; RC=1; fi
curl -fs http://127.0.0.1:8097/health >/dev/null || { echo "server unhealthy after the test" >&2; RC=1; }
grep -c '"level":"ERROR"' "$OUT/server.log" | sed 's/^/server error log lines: /'
grep -ciE 'panicked|panic' "$OUT/server.log" | sed 's/^/server panics: /'
echo "results in $OUT"
exit "$RC"
