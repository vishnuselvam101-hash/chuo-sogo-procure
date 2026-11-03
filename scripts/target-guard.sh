#!/usr/bin/env bash
# Keeps target/ from filling the disk. If target/ is over PROCURE_TARGET_LIMIT_GB (default 15),
# removes the dev profile (target/debug: test and check builds, which grow fastest and rebuild
# in minutes). Release, installer and cross-compile dirs are left alone; clean those by hand.
# Skips while another cargo build or test that uses target/debug is running.
set -uo pipefail
cd "$(dirname "$0")/.."
LIMIT_GB="${PROCURE_TARGET_LIMIT_GB:-15}"
[ -d target ] || exit 0
used_gb=$(( $(du -sk target 2>/dev/null | cut -f1) / 1024 / 1024 ))
[ "$used_gb" -le "$LIMIT_GB" ] && exit 0
if ps -eo args | grep -E '^[^ ]*cargo (test|build|check|clippy|run)' | grep -vq -- '--release'; then
  echo "target-guard: target/ is ${used_gb} GB (limit ${LIMIT_GB}); a debug build is running, not cleaning" >&2
  exit 0
fi
before=$(du -sh target/debug 2>/dev/null | cut -f1)
rm -rf target/debug
echo "target-guard: target/ was ${used_gb} GB (limit ${LIMIT_GB}); removed target/debug (${before:-0})" >&2
