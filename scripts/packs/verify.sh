#!/usr/bin/env bash
# Check every file in packs/manifest.json against its size and SHA-256 in the pack root,
# and that repo-authored files still match the manifest (rebuild it with build-manifest.py).
# Usage: scripts/packs/verify.sh [pack-id ...]     env: PACK_ROOT (default /srv/procure-packs)
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
MANIFEST="$REPO/packs/manifest.json"
ROOT="${PACK_ROOT:-/srv/procure-packs}"
want_ids=("$@")
bad=0; n=0
while IFS='|' read -r id path repo_path sha size; do
  if [ ${#want_ids[@]} -gt 0 ] && [[ ! " ${want_ids[*]} " == *" $id "* ]]; then continue; fi
  n=$((n+1))
  for f in "$ROOT/$path" ${repo_path:+"$REPO/$repo_path"}; do
    if [ ! -f "$f" ]; then echo "MISSING $f"; bad=$((bad+1)); continue; fi
    if [ "$(stat -c %s "$f")" != "$size" ] || [ "$(sha256sum "$f" | cut -c1-64)" != "$sha" ]; then
      echo "BAD     $f"; bad=$((bad+1))
    fi
  done
done < <(jq -r '.packs[] | .id as $id | (((.files // []) + [.variants[]?.files[]]) | unique_by(.path))[] | [$id, .path, (.repo_path // ""), .sha256, (.size|tostring)] | join("|")' "$MANIFEST")
echo "verify: $n files, $bad problems (root $ROOT)"
[ "$bad" -eq 0 ]
