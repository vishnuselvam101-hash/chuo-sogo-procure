#!/usr/bin/env bash
# Builds the trial installers on this server instead of GitHub Actions: Windows (cross-compiled
# with cargo-xwin; the Microsoft SDK licence was accepted by the founder on 2026-11-01), Linux
# AppImage and Android APK, for the business (Procure) and Shop apps, all preset to the demo
# server. macOS cannot be built here: the newest Mac build from a GitHub run is reused (pass its
# desktop run ids, or omit to keep the Mac files of the current package).
#   scripts/demo/build-trial-local.sh [mac-business-run mac-shop-run]
# Then it assembles the package with make-trial-package.sh --from <dir> (address check, zips,
# one-click Windows setup, guides). Separate target dirs (target/trial-*) keep dev builds untouched.
# Needs: lld/llvm (lld-link, llvm-rc), cargo-xwin, makensis, Android SDK + NDK, JDK 17+.
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT="$PWD"
SERVER="${DEMO_URL:-https://vnx-hostinger.taild091df.ts.net:8443}"
OUT="$ROOT/target/trial-out"
export PATH="/opt/rust/cargo/bin:$PATH"
export ANDROID_HOME="${ANDROID_HOME:-/opt/android-sdk}"
export NDK_HOME="${NDK_HOME:-$ANDROID_HOME/ndk/28.2.13676358}"
export XWIN_ACCEPT_LICENSE=1 XWIN_CACHE_DIR="$ROOT/target/xwin-cache"
export PROCURE_DEFAULT_SERVER="$SERVER"
rm -rf "$OUT"
mkdir -p "$OUT"/{desk-business,desk-shop,mob-business,mob-shop}

cd apps/desktop
config() { [ "$1" = shop ] && echo "--config src-tauri/tauri.shop.conf.json" || true; }

for app in business shop; do
  # shellcheck disable=SC2046
  echo "== $app: Linux AppImage"
  CARGO_TARGET_DIR="$ROOT/target/trial-linux-$app" cargo tauri build $(config $app) --bundles appimage >/dev/null
  cp "$ROOT"/target/trial-linux-$app/release/bundle/appimage/*.AppImage "$OUT/desk-$app/"

  echo "== $app: Windows setup (cross-compiled)"
  CARGO_TARGET_DIR="$ROOT/target/trial-win-$app" cargo tauri build $(config $app) \
    --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis >/dev/null
  cp "$ROOT"/target/trial-win-$app/x86_64-pc-windows-msvc/release/bundle/nsis/*-setup.exe "$OUT/desk-$app/"

  echo "== $app: Android APK"
  # Gradle does not pass the environment through to cargo; build.rs reads this file instead.
  printf '%s' "$SERVER" > src-tauri/default-server.txt
  rm -rf src-tauri/gen/android
  cargo tauri android init --ci $(config $app) >/dev/null
  cargo tauri android build --apk --target aarch64 $(config $app) >/dev/null
  rm -f src-tauri/default-server.txt
  mapfile -t apks < <(find src-tauri/gen/android/app/build/outputs/apk -name '*-unsigned.apk')
  [ "${#apks[@]}" -eq 1 ] || { echo "expected one unsigned APK, found ${#apks[@]}" >&2; exit 1; }
  # Throwaway test key, the same trust level as a debug build (store releases need a real key).
  BT="$(ls -d "$ANDROID_HOME"/build-tools/* | sort -V | tail -1)"
  KS="$(mktemp -u)"
  keytool -genkeypair -keystore "$KS" -storepass android -keypass android -alias test -keyalg RSA \
    -keysize 2048 -validity 365 -dname "CN=Chuo-Sogo test build" >/dev/null 2>&1
  "$BT/zipalign" -f -p 4 "${apks[0]}" "$OUT/aligned.apk"
  "$BT/apksigner" sign --ks "$KS" --ks-pass pass:android --out "$OUT/mob-$app/chuo-sogo-$app.apk" "$OUT/aligned.apk"
  "$BT/apksigner" verify "$OUT/mob-$app/chuo-sogo-$app.apk"
  rm -f "$KS" "$OUT/aligned.apk"
done
cd "$ROOT"

# macOS: from GitHub runs if given, else the Mac files of the current package.
if [ $# -eq 2 ]; then
  for pair in "business:$1" "shop:$2"; do
    d="$(mktemp -d)"; gh run download "${pair#*:}" -D "$d"
    find "$d" -name '*.dmg' -exec cp {} "$OUT/desk-${pair%%:*}/" \;
    rm -rf "$d"
  done
else
  PREV="reports/builds/$(sed -n 's/^version = "\(.*\)"/\1/p' apps/desktop/src-tauri/Cargo.toml | head -1)/trial/ChuoSogo-Trial-"*/Mac
  cp $PREV/ChuoSogo-Procure.dmg "$OUT/desk-business/"
  cp $PREV/ChuoSogo-Shop.dmg "$OUT/desk-shop/"
  echo "note: Mac files reused from the previous package (built from an earlier commit)"
fi
scripts/demo/make-trial-package.sh --from "$OUT"
