#!/usr/bin/env bash
# Assembles the trial package for people testing against the demo server: the guide page plus
# the Windows, Mac and Android installers of the business (Procure) and Shop apps, downloaded
# from finished GitHub runs that were dispatched with server=<demo address>.
#   scripts/demo/make-trial-package.sh <desktop-business> <desktop-shop> <mobile-business> <mobile-shop>
# (run ids), or --from <dir> for local builds (scripts/demo/build-trial-local.sh): a directory
# with desk-business, desk-shop, mob-business and mob-shop. Output: reports/builds/<version>/trial/ — a one-click Windows setup
# (trial-installer.nsi), one zip per platform and one with all.
# Every installer is checked to contain the demo address (DEMO_URL) before it is packaged, and
# the commit each run was built from is recorded in BUILD-INFO.txt.
# The guide holds no passwords; the shared demo password is handed over separately.
set -euo pipefail
cd "$(dirname "$0")/../.."
[ $# -eq 4 ] || { [ $# -eq 2 ] && [ "$1" = --from ]; } || { sed -n 2,9p "$0" >&2; exit 2; }
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' apps/desktop/src-tauri/Cargo.toml | head -1)"
OUT="reports/builds/$VERSION/trial"
DEMO_URL="${DEMO_URL:-https://vnx-hostinger.taild091df.ts.net:8443}"
DL="$(mktemp -d)"
trap 'rm -rf "$DL"' EXIT

if [ "$1" = --from ]; then
  for d in desk-business desk-shop mob-business mob-shop; do cp -r "$2/$d" "$DL/$d"; done
else
  gh run download "$1" -D "$DL/desk-business"
  gh run download "$2" -D "$DL/desk-shop"
  gh run download "$3" -D "$DL/mob-business"
  gh run download "$4" -D "$DL/mob-shop"
fi

PKG="$OUT/ChuoSogo-Trial-$VERSION"
rm -rf "$OUT"
mkdir -p "$PKG/Windows" "$PKG/Mac" "$PKG/Android" "$PKG/Linux"
one() { # exactly one file matching the pattern under a directory
  local found
  mapfile -t found < <(find "$1" -type f -name "$2")
  [ "${#found[@]}" -eq 1 ] || { echo "expected one $2 under $1, found ${#found[@]}" >&2; exit 1; }
  printf '%s' "${found[0]}"
}
for app in business:Procure shop:Shop; do
  kind="${app%%:*}" name="${app##*:}"
  cp "$(one "$DL/desk-$kind" '*-setup.exe')" "$PKG/Windows/ChuoSogo-$name-Setup.exe"
  cp "$(one "$DL/desk-$kind" '*.dmg')" "$PKG/Mac/ChuoSogo-$name.dmg"
  cp "$(one "$DL/mob-$kind" '*.apk')" "$PKG/Android/ChuoSogo-$name.apk"
  cp "$(one "$DL/desk-$kind" '*.AppImage')" "$PKG/Linux/ChuoSogo-$name.AppImage"
done
# Guides in Japanese and English (HTML to open, PDF to print or forward).
python3 scripts/demo/make-guides.py "$DL/guides" >/dev/null
cp "$DL/guides/guide-ja.html" "$PKG/START-HERE.html"
cp "$DL/guides/guide-en.html" "$PKG/START-HERE-EN.html"
cp "$DL/guides/"ChuoSogo-Trial-Guide-*.pdf "$PKG/"

# The demo address must be inside every app (an installer without it would connect nowhere).
for f in "$PKG"/Windows/*.exe "$PKG"/Mac/*.dmg "$PKG"/Android/*.apk "$PKG"/Linux/*.AppImage; do
  x="$(mktemp -d "$DL/x.XXXXXX")"
  7z x -y -o"$x" "$f" >/dev/null
  # The Windows setup holds the app exe in its own archive layer.
  find "$x" -name '*.exe' -size +1M -exec 7z x -y -o"$x/inner" {} \; >/dev/null 2>&1 || true
  grep -rqaF "$DEMO_URL" "$x" || { echo "no $DEMO_URL inside $f" >&2; exit 1; }
  echo "checked: $(basename "$f") contains $DEMO_URL"
done
if [ "$1" = --from ]; then
  echo "built on this server from $(git rev-parse --short HEAD)$(git diff --quiet || echo ' (uncommitted changes)'), $(date -u +%FT%TZ)" > "$OUT/BUILD-INFO.txt"
else
  for id in "$@"; do
    gh run view "$id" --json databaseId,workflowName,headBranch,headSha,conclusion \
      -q '"run \(.databaseId) \(.workflowName) \(.headBranch)@\(.headSha[0:7]) \(.conclusion)"'
  done > "$OUT/BUILD-INFO.txt"
fi
echo "demo server: $DEMO_URL" >> "$OUT/BUILD-INFO.txt"

python3 - "$OUT" "ChuoSogo-Trial-$VERSION" <<'EOF'
import os, sys, zipfile
out, top = sys.argv[1], sys.argv[2]
root = os.path.join(out, top)
def pack(name, parts):
    with zipfile.ZipFile(os.path.join(out, name), "w", zipfile.ZIP_DEFLATED) as z:
        for f in sorted(os.listdir(root)):
            if os.path.isfile(os.path.join(root, f)):
                z.write(os.path.join(root, f), f"{top}/{f}")
        for part in parts:
            for f in sorted(os.listdir(os.path.join(root, part))):
                z.write(os.path.join(root, part, f), f"{top}/{part}/{f}")
for part in ("Windows", "Mac", "Android", "Linux"):
    pack(f"{top}-{part}.zip", [part])
pack(f"{top}-All.zip", ["Windows", "Mac", "Android", "Linux"])
EOF
# Windows: one installer for both apps and the guide.
makensis -V2 -DVERSION="$VERSION" -DPKG="$PWD/$PKG" -DOUTFILE="$PWD/$OUT/ChuoSogo-Trial-$VERSION-Setup.exe" \
  -DICON="$PWD/apps/desktop/src-tauri/icons/icon.ico" scripts/demo/trial-installer.nsi
( cd "$OUT" && sha256sum ./*.zip ./*.exe > SHA256SUMS )
ls -la "$OUT"
