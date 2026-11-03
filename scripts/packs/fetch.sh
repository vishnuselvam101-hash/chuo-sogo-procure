#!/usr/bin/env bash
# Stage every pack file listed in packs/manifest.json into the pack root, verifying SHA-256.
# Idempotent: a file already present with the right size and hash is skipped.
# Resumable: downloads go to <file>.part and continue with `curl -C -`.
# Upstream files come from `upstream_url`; repo-authored files are copied from `repo_path`.
# Usage: scripts/packs/fetch.sh [pack-id ...]     env: PACK_ROOT (default /srv/procure-packs)
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
MANIFEST="$REPO/packs/manifest.json"
ROOT="${PACK_ROOT:-/srv/procure-packs}"
mkdir -p "$ROOT"

want_ids=("$@")

hash_of() { sha256sum "$1" | cut -c1-64; }

jq -r '.packs[] | .id as $id | (((.files // []) + [.variants[]?.files[]]) | unique_by(.path))[] | [$id, .path, (.upstream_url // ""), (.repo_path // ""), .sha256, (.size|tostring), (.repack.script // ""), (.repack.sha256 // "")] | join("|")' "$MANIFEST" |
while IFS='|' read -r id path url repo_path sha size repack repack_sha; do
  if [ ${#want_ids[@]} -gt 0 ] && [[ ! " ${want_ids[*]} " == *" $id "* ]]; then continue; fi
  dest="$ROOT/$path"
  mkdir -p "$(dirname "$dest")"
  if [ -f "$dest" ] && [ "$(stat -c %s "$dest")" = "$size" ] && [ "$(hash_of "$dest")" = "$sha" ]; then
    echo "ok      $path"; continue
  fi
  if [ -n "$repo_path" ]; then
    cp "$REPO/$repo_path" "$dest.part"
  elif [ -n "$repack" ]; then
    # Upstream file (pinned) -> repo script -> pinned result.
    src="$ROOT/.upstream/$(basename "$url")"
    mkdir -p "$ROOT/.upstream"
    [ -f "$src" ] && [ "$(hash_of "$src")" = "$repack_sha" ] ||
      curl -fsSL --proto '=https' --retry 5 -o "$src" "$url" || { echo "FAIL    $path: download" >&2; continue; }
    [ "$(hash_of "$src")" = "$repack_sha" ] || { echo "FAIL    $path: upstream sha" >&2; rm -f "$src"; continue; }
    echo "repack  $path"
    python3 -I "$REPO/$repack" "$src" "$dest.part" >/dev/null || { echo "FAIL    $path: repack" >&2; continue; }
  elif [ -f "$dest.part" ] && [ "$(stat -c %s "$dest.part")" = "$size" ]; then
    echo "check   $path (complete .part)"
  else
    avail=$(df --output=avail -B1 "$ROOT" | tail -1)
    if [ "$avail" -lt $((size + 5*1024*1024*1024)) ]; then
      echo "SKIP    $path: needs $size bytes + 5 GB headroom, only $avail free" >&2; continue
    fi
    echo "fetch   $path"
    quiet=(); [ -t 2 ] || quiet=(-sS)
    curl -fL "${quiet[@]}" --retry 5 --retry-delay 3 -C - -o "$dest.part" "$url" || { echo "FAIL    $path: download" >&2; continue; }
  fi
  got_size=$(stat -c %s "$dest.part"); got_sha=$(hash_of "$dest.part")
  if [ "$got_size" = "$size" ] && [ "$got_sha" = "$sha" ]; then
    mv -f "$dest.part" "$dest"; echo "ok      $path (new)"
  else
    echo "FAIL    $path: size $got_size/$size sha $got_sha" >&2
    if [ "$got_size" -ge "$size" ]; then rm -f "$dest.part"; fi   # complete but wrong: restart next time
  fi
done
# the while loop runs in a subshell; re-check the result with verify.sh
exec "$REPO/scripts/packs/verify.sh" "$@"
