#!/usr/bin/env bash
# Restore drill: proves a backup can be restored. Checks the checksums, restores the database
# into a throwaway database on the same server, unpacks the files into a temporary directory
# and runs `procure-server verify` (audit chains + every stored file). Nothing existing is
# touched; the throwaway database and files are removed afterwards. Run as root: restoring
# rows into tables with forced row-level security needs the PostgreSQL superuser (local
# socket, `runuser -u postgres`); `verify` then runs as the owner.
#   scripts/restore-drill.sh <backup-dir> [env-file]   (env-file: DATABASE_URL, default ./.env)
set -euo pipefail
backup="${1:?usage: restore-drill.sh <backup-dir> [env-file]}"
envfile="${2:-./.env}"
set -a; . "$envfile"; set +a
bin="${PROCURE_SERVER_BIN:-target/release/procure-server}"
[ -x "$bin" ] || bin=/opt/procure/bin/procure-server
(cd "$backup" && sha256sum --check --quiet SHA256SUMS)
db="procure_restore_$(date -u +%Y%m%d%H%M%S)"
work="$(mktemp -d)"
cleanup() {
  psql "$DATABASE_URL" -qc "DROP DATABASE IF EXISTS $db WITH (FORCE)" >/dev/null 2>&1 \
    || runuser -u postgres -- psql -qc "DROP DATABASE IF EXISTS $db WITH (FORCE)" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT
psql "$DATABASE_URL" -qc "CREATE DATABASE $db"
target="${DATABASE_URL%/*}/$db"
# Read by root and handed over on stdin: the dump never becomes readable by another user.
runuser -u postgres -- pg_restore --exit-on-error --dbname="$db" < "$backup/db.dump"
mkdir -p "$work/storage"
tar -C "$work/storage" -xf "$backup/storage.tar"
DATABASE_URL="$target" PROCURE_STORAGE_DIR="$work/storage" "$bin" verify
echo "restore drill passed for $backup"
