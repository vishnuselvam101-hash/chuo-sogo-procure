#!/usr/bin/env bash
# One-time, as root, on a development machine: creates the procure_backup role (read-only,
# BYPASSRLS — the only way to dump tables whose row-level security is forced even for the
# owner) and records PROCURE_BACKUP_DATABASE_URL in ./.env. Production gets the same role from
# scripts/install-server.sh (URL in /etc/procure/procure.env). The password is never printed.
set -euo pipefail
cd "$(dirname "$0")/.."
grep -q '^PROCURE_BACKUP_DATABASE_URL=' .env && { echo "already set in .env"; exit 0; }
PW="$(openssl rand -hex 24)"
runuser -u postgres -- psql -v ON_ERROR_STOP=1 -q -v backup_pw="$PW" <<'SQL'
SELECT 'CREATE ROLE procure_backup LOGIN NOSUPERUSER BYPASSRLS NOCREATEDB NOCREATEROLE' WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'procure_backup') \gexec
ALTER ROLE procure_backup PASSWORD :'backup_pw';
GRANT pg_read_all_data TO procure_backup;
SQL
URL="$(grep '^DATABASE_URL=' .env | cut -d= -f2- | sed -E "s#//[^:]+:[^@]+@#//procure_backup:${PW}@#")"
echo "PROCURE_BACKUP_DATABASE_URL=$URL" >> .env
echo "procure_backup ready"
