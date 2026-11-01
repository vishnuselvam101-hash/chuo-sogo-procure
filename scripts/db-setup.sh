#!/usr/bin/env bash
# One-time PostgreSQL role setup (run as a PostgreSQL superuser, e.g. `sudo -u postgres scripts/db-setup.sh`).
#   procure          LOGIN owner: runs migrations, owns tables, CREATEDB for the test suite.
#   procure_runtime  NOLOGIN group: the only privileges the running server needs (DML, no DDL,
#                    no UPDATE/DELETE on audit_events). Granted by the migrations.
#   procure_app      LOGIN member of procure_runtime: what the server connects as.
#   procure_backup   LOGIN, read-only (pg_read_all_data), BYPASSRLS: only for pg_dump. Row-level
#                    security is forced even for the table owner, so nothing else can make a
#                    complete backup. Optional: created when PROCURE_BACKUP_PASSWORD is set.
# Passwords are read from the environment and never echoed.
set -euo pipefail
: "${PROCURE_OWNER_PASSWORD:?set PROCURE_OWNER_PASSWORD}" "${PROCURE_APP_PASSWORD:?set PROCURE_APP_PASSWORD}"
psql -v ON_ERROR_STOP=1 -q \
  -v owner_pw="$PROCURE_OWNER_PASSWORD" -v app_pw="$PROCURE_APP_PASSWORD" <<'SQL'
SELECT 'CREATE ROLE procure LOGIN CREATEDB NOSUPERUSER NOBYPASSRLS' WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'procure') \gexec
SELECT 'CREATE ROLE procure_runtime NOLOGIN NOSUPERUSER NOBYPASSRLS' WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'procure_runtime') \gexec
SELECT 'CREATE ROLE procure_app LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB' WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'procure_app') \gexec
ALTER ROLE procure PASSWORD :'owner_pw';
ALTER ROLE procure_app PASSWORD :'app_pw';
GRANT procure_runtime TO procure_app;
-- Lets the test suite run as the runtime role (SET ROLE) inside its own throwaway databases.
GRANT procure_runtime TO procure;
SELECT 'CREATE DATABASE procure_dev OWNER procure ENCODING ''UTF8'' TEMPLATE template0' WHERE NOT EXISTS (SELECT 1 FROM pg_database WHERE datname = 'procure_dev') \gexec
SQL
if [ -n "${PROCURE_BACKUP_PASSWORD:-}" ]; then
  psql -v ON_ERROR_STOP=1 -q -v backup_pw="$PROCURE_BACKUP_PASSWORD" <<'SQL'
SELECT 'CREATE ROLE procure_backup LOGIN NOSUPERUSER BYPASSRLS NOCREATEDB NOCREATEROLE' WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'procure_backup') \gexec
ALTER ROLE procure_backup PASSWORD :'backup_pw';
GRANT pg_read_all_data TO procure_backup;
SQL
fi
echo "roles ready"
