#!/usr/bin/env bash
# Off-site copy of the newest nightly backup (B6). Off unless configured: with neither
# PROCURE_OFFSITE_TARGET nor PROCURE_OFFSITE_GPG_RECIPIENT set it says so and exits 0.
#
# The backup run (db.dump + storage.tar + SHA256SUMS) is checked, packed, encrypted on this
# server to the founder's public key (the private key never lives on the server, so the
# off-site copy cannot be read by the storage provider or by someone who takes this machine),
# uploaded with rclone (S3-compatible object storage in Japan, SFTP, or a local path), read
# back and compared by SHA-256, and the newest PROCURE_OFFSITE_KEEP copies are kept.
#
# Settings (/etc/procure/procure.env):
#   PROCURE_OFFSITE_TARGET         rclone destination, e.g. "jp-backup:procure" or a directory
#   PROCURE_OFFSITE_GPG_RECIPIENT  fingerprint of the public key to encrypt to
#   PROCURE_OFFSITE_GNUPGHOME      keyring holding that public key (default /etc/procure/gnupg)
#   RCLONE_CONFIG                  rclone config with the storage credentials
#   PROCURE_OFFSITE_KEEP           copies kept off-site (default 30)
#   PROCURE_BACKUP_DIR             local backups (default /var/lib/procure/backups)
# Restore: scripts/procure-offsite-restore.sh (needs the private key, held offline).
set -euo pipefail
umask 077

target="${PROCURE_OFFSITE_TARGET:-}"
recipient="${PROCURE_OFFSITE_GPG_RECIPIENT:-}"
if [[ -z "$target" || -z "$recipient" ]]; then
  echo "off-site backup not configured (PROCURE_OFFSITE_TARGET, PROCURE_OFFSITE_GPG_RECIPIENT); nothing done"
  exit 0
fi
root="${PROCURE_BACKUP_DIR:-/var/lib/procure/backups}"
keep="${PROCURE_OFFSITE_KEEP:-30}"
[[ "$keep" =~ ^[1-9][0-9]*$ ]] || { echo "PROCURE_OFFSITE_KEEP must be a positive number" >&2; exit 2; }
export GNUPGHOME="${PROCURE_OFFSITE_GNUPGHOME:-/etc/procure/gnupg}"

shopt -s nullglob
runs=("$root"/2*/)
if (( ${#runs[@]} == 0 )); then
  echo "no local backup in $root" >&2
  exit 1
fi
# Glob results are sorted, and run names are UTC timestamps: the last is the newest.
run="${runs[-1]%/}"
name="$(basename "$run")"
object="procure-$name.tar.gpg"

if rclone lsf "$target" --files-only 2>/dev/null | grep -qxF "$object"; then
  echo "$object already off-site; nothing to do"
  exit 0
fi

# A backup that fails its own checksums is not shipped.
(cd "$run" && sha256sum --quiet -c SHA256SUMS)

work="$(mktemp -d "${TMPDIR:-/var/lib/procure/work}/offsite.XXXXXX")"
trap 'rm -rf "$work"' EXIT
tar -C "$root" -cf - "$name" \
  | gpg --batch --yes --quiet --trust-model always --recipient "$recipient" \
        --encrypt --output "$work/$object"
(cd "$work" && sha256sum "$object" > "$object.sha256")
want="$(cut -d' ' -f1 "$work/$object.sha256")"

rclone copyto "$work/$object" "$target/$object"
rclone copyto "$work/$object.sha256" "$target/$object.sha256"
# Read the copy back: an upload is only counted once it is byte-for-byte what was sent.
got="$(rclone cat "$target/$object" | sha256sum | cut -d' ' -f1)"
if [[ "$got" != "$want" ]]; then
  echo "verification failed for $target/$object (sha256 $got, expected $want)" >&2
  rclone deletefile "$target/$object" || true
  exit 1
fi

# Retention: newest $keep encrypted copies (names sort by time).
mapfile -t copies < <(rclone lsf "$target" --files-only | grep -E '^procure-2[0-9TZ]+\.tar\.gpg$' | sort)
if (( ${#copies[@]} > keep )); then
  for old in "${copies[@]:0:${#copies[@]}-keep}"; do
    rclone deletefile "$target/$old"
    rclone deletefile "$target/$old.sha256" || true
  done
fi
echo "off-site copy verified: $target/$object (sha256 $want)"
