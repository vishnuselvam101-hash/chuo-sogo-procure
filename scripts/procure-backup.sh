#!/usr/bin/env bash
# Nightly backup (installed as /opt/procure/bin/procure-backup, run by procure-backup.timer):
# the database (pg_dump custom format) and the stored files (attachments, listing photos,
# invoice PDFs), with SHA-256 checksums, in one directory per run; the newest 14 are kept.
# Reads PROCURE_BACKUP_DATABASE_URL and PROCURE_STORAGE_DIR from /etc/procure/procure.env.
set -euo pipefail
umask 077
root="${PROCURE_BACKUP_DIR:-/var/lib/procure/backups}"
dest="$root/$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -p "$dest"
# A role with BYPASSRLS (row-level security is forced even for the owner, so an owner dump
# would fail rather than silently miss rows).
pg_dump --format=custom --file="$dest/db.dump" "${PROCURE_BACKUP_DATABASE_URL:?PROCURE_BACKUP_DATABASE_URL is not set (see scripts/install-server.sh)}"
tar -C "${PROCURE_STORAGE_DIR:?}" -cf "$dest/storage.tar" .
(cd "$dest" && sha256sum db.dump storage.tar > SHA256SUMS)
# Retention: newest 14 runs (older single-file dumps from earlier versions as well). A glob
# with no match is not an error here.
shopt -s nullglob
runs=("$root"/2*/)
if (( ${#runs[@]} > 14 )); then
  ls -1dt "${runs[@]}" | tail -n +15 | xargs -r rm -rf
fi
dumps=("$root"/procure-*.dump)
if (( ${#dumps[@]} > 14 )); then
  ls -1t "${dumps[@]}" | tail -n +15 | xargs -r rm -f
fi
echo "backup written to $dest"
