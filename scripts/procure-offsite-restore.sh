#!/usr/bin/env bash
# Fetches one off-site backup copy, decrypts it and checks it (B6).
#   scripts/procure-offsite-restore.sh <target> <copy-name|latest> <output-dir>
# Needs the private key in GNUPGHOME (held offline by the founder, never on the server).
# The result is a backup run directory (db.dump, storage.tar, SHA256SUMS) for
# scripts/restore-drill.sh or a manual restore (docs/operations/OPERATIONS.md).
set -euo pipefail
umask 077
target="${1:?target}"
copy="${2:?copy name or latest}"
out="${3:?output directory}"
if [[ "$copy" == latest ]]; then
  copy="$(rclone lsf "$target" --files-only | grep -E '^procure-2[0-9TZ]+\.tar\.gpg$' | sort | tail -n 1)"
  [[ -n "$copy" ]] || { echo "no copies in $target" >&2; exit 1; }
fi
mkdir -p "$out"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
rclone copyto "$target/$copy" "$work/$copy"
rclone copyto "$target/$copy.sha256" "$work/$copy.sha256"
(cd "$work" && sha256sum --quiet -c "$copy.sha256")
gpg --batch --quiet --decrypt "$work/$copy" | tar -C "$out" -xf -
run="$out/${copy#procure-}"
run="${run%.tar.gpg}"
(cd "$run" && sha256sum --quiet -c SHA256SUMS)
echo "restored and verified: $run"
