#!/usr/bin/env bash
# OWASP ZAP API scan (active) of a throwaway local server, never production or third-party
# systems: throwaway database, server on 127.0.0.1:8088, the OpenAPI document as the map of the
# API, and a signed-in session (bootstrap administrator) so authenticated routes are scanned
# too. Report in reports/zap-<time>/. Needs docker with ghcr.io/zaproxy/zaproxy:stable.
set -euo pipefail
cd "$(dirname "$0")/.."
set -a; . ./.env; set +a
OUT="reports/zap-$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -p "$OUT"
chmod 777 "$OUT"   # the ZAP container writes its report as its own user
WORK="$(mktemp -d)"
DB="procure_zap_$(date -u +%Y%m%d%H%M%S)"
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
if curl -fs http://127.0.0.1:8088/health >/dev/null; then
  echo "port 8088 is in use; stop the dev server first" >&2
  exit 1
fi
psql "$DATABASE_URL" -qc "CREATE DATABASE $DB"
TOKEN="$(DATABASE_URL="$OWNER_URL" target/release/procure-server bootstrap "Chuo-Sogo Inc." root@chuo-sogo.zap "Root" | tail -1)"
DATABASE_URL="$OWNER_URL" PROCURE_RUNTIME_DATABASE_URL="$RUNTIME_URL" PROCURE_OUTBOX_DIR="$WORK/outbox" \
  PROCURE_WORK_DIR="$WORK/work" PROCURE_STORAGE_DIR="$WORK/storage" PROCURE_BIND=127.0.0.1:8088 \
  target/release/procure-server serve >"$OUT/server.log" 2>&1 &
PID=$!
for _ in $(seq 40); do curl -fs http://127.0.0.1:8088/health >/dev/null && break; sleep 0.25; done
PASSWORD="zap-correct-horse-battery-$RANDOM"
curl -fsS -H 'content-type: application/json' -d "{\"token\":\"$TOKEN\",\"password\":\"$PASSWORD\"}" \
  http://127.0.0.1:8088/api/v1/auth/set-password >/dev/null
SESSION="$(curl -fsS -H 'content-type: application/json' \
  -d "{\"email\":\"root@chuo-sogo.zap\",\"password\":\"$PASSWORD\"}" \
  http://127.0.0.1:8088/api/v1/auth/login | sed -E 's/.*"token":"([^"]+)".*/\1/')"
cp api/openapi.json "$OUT/openapi.json"
# -O: target base URL; the replacer adds the session to every request ZAP sends.
docker run --rm --network host -v "$PWD/$OUT:/zap/wrk:rw" ghcr.io/zaproxy/zaproxy:stable \
  zap-api-scan.py -t /zap/wrk/openapi.json -f openapi -O http://127.0.0.1:8088 \
  -r zap-report.html -J zap-report.json -w zap-report.md -I \
  -z "-config replacer.full_list(0).description=auth -config replacer.full_list(0).enabled=true \
      -config replacer.full_list(0).matchtype=REQ_HEADER -config replacer.full_list(0).matchstr=Authorization \
      -config replacer.full_list(0).regex=false -config replacer.full_list(0).replacement=Bearer\ $SESSION" \
  | tee "$OUT/zap.log" | tail -25
grep -c '" 500 ' "$OUT/server.log" >/dev/null 2>&1 || true
echo "report: $OUT/zap-report.md"
