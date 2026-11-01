#!/usr/bin/env bash
# Publishes a trial package (scripts/demo/make-trial-package.sh) to the public download folder
# served by Tailscale Funnel at <demo address>/downloads/ (run as root):
#   tailscale funnel --bg --https=8443 --set-path=/downloads /srv/procure-trial
# File names are stable, so the website's trial page (vsnexus-web src/content/trial.ts) keeps
# working; it prints the sizes and SHA-256 to paste into that file. Old files are replaced.
set -euo pipefail
cd "$(dirname "$0")/../.."
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' apps/desktop/src-tauri/Cargo.toml | head -1)"
PKG="reports/builds/$VERSION/trial"
SRC="$PKG/ChuoSogo-Trial-$VERSION"
DEST="${1:-/srv/procure-trial}"
[ -d "$SRC" ] || { echo "no package at $SRC; run make-trial-package.sh first" >&2; exit 1; }
install -d -m 0755 "$DEST"
declare -A MAP=(
  [win]="$PKG/ChuoSogo-Trial-$VERSION-Setup.exe:ChuoSogo-Trial-Setup.exe"
  [mac-procure]="$SRC/Mac/ChuoSogo-Procure.dmg:ChuoSogo-Procure.dmg"
  [mac-shop]="$SRC/Mac/ChuoSogo-Shop.dmg:ChuoSogo-Shop.dmg"
  [android-procure]="$SRC/Android/ChuoSogo-Procure.apk:ChuoSogo-Procure.apk"
  [android-shop]="$SRC/Android/ChuoSogo-Shop.apk:ChuoSogo-Shop.apk"
  [linux-procure]="$SRC/Linux/ChuoSogo-Procure.AppImage:ChuoSogo-Procure.AppImage"
  [linux-shop]="$SRC/Linux/ChuoSogo-Shop.AppImage:ChuoSogo-Shop.AppImage"
)
# Copy under a temporary name, then rename: a download in progress never sees half a file.
for id in "${!MAP[@]}"; do
  from="${MAP[$id]%%:*}" to="${MAP[$id]##*:}"
  install -m 0644 "$from" "$DEST/.$to.new"
  mv -f "$DEST/.$to.new" "$DEST/$to"
done
# Guides: HTML to read in the browser, PDF to print or forward.
install -m 0644 "$SRC/START-HERE.html" "$DEST/guide-ja.html"
install -m 0644 "$SRC/START-HERE-EN.html" "$DEST/guide-en.html"
install -m 0644 "$SRC"/ChuoSogo-Trial-Guide-ja.pdf "$SRC"/ChuoSogo-Trial-Guide-en.pdf "$DEST/"
rm -f "$DEST/guide.html"
# Remove installers that are no longer published (older names).
keep=" "; for id in "${!MAP[@]}"; do keep+="${MAP[$id]##*:} "; done
shopt -s nullglob
for f in "$DEST"/*.apk "$DEST"/*.exe "$DEST"/*.dmg "$DEST"/*.AppImage; do
  [[ "$keep" == *" $(basename "$f") "* ]] || rm -f "$f"
done
for id in win mac-procure mac-shop android-procure android-shop linux-procure linux-shop; do
  to="${MAP[$id]##*:}"
  printf "%s size=%s sha256=%s\n" "$id" "$(stat -c %s "$DEST/$to")" "$(sha256sum "$DEST/$to" | cut -d' ' -f1)"
done
