#!/usr/bin/env bash
# Desktop E2E smoke test of the real release binary (Linux, headless).
# Needs: a backend at http://127.0.0.1:8088, an active user (E2E_EMAIL, E2E_PASSWORD),
# a release build (cargo tauri build), tauri-driver, WebKitWebDriver and Xvfb.
# Uses a throwaway HOME so the developer's own preferences are never touched.
set -euo pipefail
cd "$(dirname "$0")/.."
: "${E2E_EMAIL:?}" "${E2E_PASSWORD:?}"
OUT="${E2E_OUT:-reports/e2e-$(date -u +%Y%m%dT%H%M%SZ)}"
mkdir -p "$OUT"
WORK="$(mktemp -d)"
DISPLAY_NUM=":98"

Xvfb "$DISPLAY_NUM" -screen 0 1400x900x24 >"$OUT/xvfb.log" 2>&1 &
XVFB=$!
sleep 1
DISPLAY="$DISPLAY_NUM" HOME="$WORK" XDG_CONFIG_HOME="$WORK/.config" WEBKIT_DISABLE_DMABUF_RENDERER=1 \
  RUST_LOG=procure_desktop_lib=debug,procure_ui=debug tauri-driver >"$OUT/app-and-driver.log" 2>&1 &
DRIVER=$!
trap 'kill $DRIVER $XVFB 2>/dev/null || true; rm -rf "$WORK"' EXIT
for _ in $(seq 30); do curl -fs http://127.0.0.1:4444/status >/dev/null && break; sleep 0.5; done

E2E_APP="$PWD/target/release/procure-desktop" E2E_OUT="$PWD/$OUT" E2E_EMAIL="$E2E_EMAIL" E2E_PASSWORD="$E2E_PASSWORD" \
  node apps/desktop/e2e/smoke.mjs | tee "$OUT/result.txt"
