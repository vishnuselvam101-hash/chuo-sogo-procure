#!/usr/bin/env bash
# Builds the full Windows installer on this server: the Procure app with standalone mode
# ("use on this PC only"), procure-server.exe, a trimmed portable PostgreSQL (official EDB
# Windows x64 binaries, PostgreSQL licence) and the Windows core packs. The large packs (AI and
# voice models) are not inside: the app downloads them from the manifest's upstream URLs.
#   scripts/demo/build-full-local.sh            -> reports/builds/<version>/full/ChuoSogo-Setup-<version>.exe
# "Connect to a server" defaults to the demo server (DEMO_URL, as in build-trial-local.sh);
# set DEMO_URL to preset another https address.
# Needs: cargo-xwin (Microsoft SDK licence accepted by the founder on 2026-11-01), lld/llvm,
# makensis, unzip, the packs in $PACK_ROOT (scripts/packs/fetch.sh). The installer is UNSIGNED.
# Nothing is uploaded or published.
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT="$PWD"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' apps/desktop/src-tauri/Cargo.toml | head -1)"
PACK_ROOT="${PACK_ROOT:-/srv/procure-packs}"
CACHE="$ROOT/target/standalone-cache"
STAGE="$ROOT/target/full-stage/windows"
OUT="$ROOT/reports/builds/$VERSION/full"
# Pinned PostgreSQL build: hash recorded at first download from get.enterprisedb.com (EDB
# publishes no digest); a changed file is refused.
PG_ZIP="postgresql-18.6-1-windows-x64-binaries.zip"
PG_URL="https://get.enterprisedb.com/postgresql/$PG_ZIP"
PG_SHA256="fbe23da234ee31547bf8a36d29dfd81e82b849df2d2b78d2eecb43d360252f8c"

export RUSTUP_HOME="${RUSTUP_HOME:-/opt/rust/rustup}" CARGO_HOME="${CARGO_HOME:-/opt/rust/cargo}"
export PATH="/opt/rust/cargo/bin:$PATH" RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-1.95}"
export XWIN_ACCEPT_LICENSE=1 XWIN_CACHE_DIR="$ROOT/target/xwin-cache"
export PROCURE_DEFAULT_SERVER="${DEMO_URL:-https://trial.vsnexustechnologies.com}"
TARGET_DIR="$ROOT/target/trial-win-business"
WIN="$TARGET_DIR/x86_64-pc-windows-msvc/release"

echo "== PostgreSQL ($PG_ZIP)"
mkdir -p "$CACHE"
if [ ! -f "$CACHE/$PG_ZIP" ]; then
  curl -fL --proto '=https' -C - -o "$CACHE/$PG_ZIP.part" "$PG_URL"
  mv "$CACHE/$PG_ZIP.part" "$CACHE/$PG_ZIP"
fi
echo "$PG_SHA256  $CACHE/$PG_ZIP" | sha256sum -c --quiet

echo "== procure-server.exe (cross-compiled)"
CARGO_TARGET_DIR="$TARGET_DIR" cargo xwin build -p procure-server --release \
  --target x86_64-pc-windows-msvc >/dev/null

echo "== staging"
rm -rf "$STAGE"
mkdir -p "$STAGE/full/bin" "$STAGE/full/packs"
cp "$WIN/procure-server.exe" "$STAGE/full/bin/"
X="$(mktemp -d)"
trap 'rm -rf "$X"' EXIT
# Only what a local database needs: no pgAdmin, StackBuilder, docs, headers, static libs or
# debug symbols.
unzip -q "$CACHE/$PG_ZIP" 'pgsql/bin/*' 'pgsql/lib/*' 'pgsql/share/*' 'pgsql/server_license.txt' \
  'pgsql/commandlinetools_3rd_party_licenses.txt' -x 'pgsql/symbols/*' 'pgsql/doc/*' -d "$X"
PG="$STAGE/full/pgsql"
mkdir -p "$PG/bin"
for t in postgres initdb pg_ctl psql pg_isready pg_dump pg_restore; do cp "$X/pgsql/bin/$t.exe" "$PG/bin/"; done
cp "$X"/pgsql/bin/*.dll "$PG/bin/"
cp -r "$X/pgsql/lib" "$X/pgsql/share" "$PG/"
find "$PG/lib" -name '*.lib' -delete
find "$PG" -name '*.pdb' -delete
rm -rf "$PG/share/doc"
cp "$X/pgsql/server_license.txt" "$X/pgsql/commandlinetools_3rd_party_licenses.txt" "$PG/"

# Core packs for Windows (and every licence file), verified against the manifest.
cp packs/manifest.json "$STAGE/full/packs/"
python3 -I - "$PACK_ROOT" "$STAGE/full/packs" packs/manifest.json <<'EOF'
import hashlib, json, os, shutil, sys
src, dst, manifest = sys.argv[1:4]
m = json.load(open(manifest))
for p in m["packs"]:
    if p["kind"] != "core":
        continue
    for f in p["files"]:
        if f.get("platform") not in (None, "windows-x64"):
            continue
        a, b = os.path.join(src, f["path"]), os.path.join(dst, f["path"])
        h = hashlib.sha256(open(a, "rb").read()).hexdigest()
        if h != f["sha256"] or os.path.getsize(a) != f["size"]:
            sys.exit(f"pack file does not match the manifest: {f['path']}")
        os.makedirs(os.path.dirname(b), exist_ok=True)
        shutil.copyfile(a, b)
print("core packs staged and verified")
EOF
du -sh "$STAGE/full" "$STAGE/full/"*

echo "== Windows setup (cross-compiled, NSIS)"
# On Linux the bundler fetches its NSIS plugin on every build, and its HTTP client times out
# on this server; fetch it with curl (same pinned SHA-1 the bundler checks) and serve it to the
# bundler from 127.0.0.1 through its documented mirror setting.
NSIS_DLL_URL="https://github.com/tauri-apps/nsis-tauri-utils/releases/download/nsis_tauri_utils-v0.5.3/nsis_tauri_utils.dll"
mkdir -p "$CACHE/nsis-mirror"
[ -f "$CACHE/nsis-mirror/nsis_tauri_utils.dll" ] ||
  curl -fsSL --proto '=https' -o "$CACHE/nsis-mirror/nsis_tauri_utils.dll" "$NSIS_DLL_URL"
echo "75197fee3c6a814fe035788d1c34ead39349b860  $CACHE/nsis-mirror/nsis_tauri_utils.dll" | sha1sum -c --quiet
MIRROR_PORT="$(python3 -I -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')"
python3 -I -m http.server "$MIRROR_PORT" --bind 127.0.0.1 --directory "$CACHE/nsis-mirror" >/dev/null 2>&1 &
MIRROR_PID=$!
trap 'rm -rf "$X"; kill "$MIRROR_PID" 2>/dev/null || true' EXIT
export TAURI_BUNDLER_TOOLS_GITHUB_MIRROR_TEMPLATE="http://127.0.0.1:$MIRROR_PORT/<asset>"
sleep 1
OVERLAY="$ROOT/target/full-stage/tauri.full.windows.json"
printf '{"bundle":{"resources":{"%s/full/":"full/"}}}\n' "$STAGE" > "$OVERLAY"
(cd apps/desktop && CARGO_TARGET_DIR="$TARGET_DIR" cargo tauri build --config "$OVERLAY" \
  --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis >/dev/null)
grep -qaF "$PROCURE_DEFAULT_SERVER" "$WIN/procure-desktop.exe" ||
  { echo "the app does not contain $PROCURE_DEFAULT_SERVER" >&2; exit 1; }
mkdir -p "$OUT"
SETUP="$OUT/ChuoSogo-Setup-$VERSION.exe"
cp "$WIN"/bundle/nsis/*_"$VERSION"_x64-setup.exe "$SETUP"
(cd "$OUT" && sha256sum "$(basename "$SETUP")" > SHA256SUMS)
{
  echo "built on this server from $(git rev-parse --short HEAD)$(git diff --quiet || echo ' (uncommitted changes)'), $(date -u +%FT%TZ)"
  echo "UNSIGNED (founder decision): Windows SmartScreen will warn before the first run."
  echo "installer: $(basename "$SETUP") $(stat -c %s "$SETUP") bytes"
  echo "PostgreSQL: $PG_ZIP sha256 $PG_SHA256"
  echo "connect-to-a-server default: $PROCURE_DEFAULT_SERVER"
} > "$OUT/BUILD-INFO.txt"
cat "$OUT/BUILD-INFO.txt"
