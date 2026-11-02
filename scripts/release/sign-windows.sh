#!/usr/bin/env bash
# Authenticode-signs Windows installers/executables (B7), with an RFC 3161 timestamp so the
# signature stays valid after the certificate expires, then verifies each one.
#   scripts/release/sign-windows.sh <file.exe> [more.exe ...]
# Signs in place (keep the unsigned build if you need it). Off until configured:
#   PROCURE_SIGN_PFX            code-signing certificate + key (.pfx/.p12) bought by the founder
#   PROCURE_SIGN_PASSWORD_FILE  file holding the PFX password (not on the command line)
#   PROCURE_SIGN_TIMESTAMP_URL  timestamp server (default http://timestamp.digicert.com)
#   PROCURE_SIGN_NAME / _URL    shown in the Windows prompt (default Chuo-Sogo Procure / site)
# EV certificates usually live on a hardware token or cloud HSM: sign on the machine with
# the token (signtool) or via the vendor's cloud service instead; see docs/operations/SIGNING.md.
set -euo pipefail
(( $# > 0 )) || { echo "usage: $0 <file.exe> [...]" >&2; exit 2; }
pfx="${PROCURE_SIGN_PFX:-}"
pass_file="${PROCURE_SIGN_PASSWORD_FILE:-}"
missing=()
[[ -n "$pfx" && -f "$pfx" ]] || missing+=("PROCURE_SIGN_PFX (certificate file)")
[[ -n "$pass_file" && -f "$pass_file" ]] || missing+=("PROCURE_SIGN_PASSWORD_FILE")
command -v osslsigncode >/dev/null || missing+=("osslsigncode (apt install osslsigncode)")
if (( ${#missing[@]} )); then
  printf 'not signing; missing: %s\n' "${missing[@]}" >&2
  exit 3
fi
ts="${PROCURE_SIGN_TIMESTAMP_URL:-http://timestamp.digicert.com}"
name="${PROCURE_SIGN_NAME:-Chuo-Sogo Procure}"
url="${PROCURE_SIGN_URL:-https://www.chuo-sogo.net}"
# Verification checks the signature and timestamp against the system CA list, or against
# PROCURE_SIGN_CAFILE (only for testing with a private test certificate).
verify_ca=()
[[ -n "${PROCURE_SIGN_CAFILE:-}" ]] && verify_ca=(-CAfile "$PROCURE_SIGN_CAFILE")
for f in "$@"; do
  [[ -f "$f" ]] || { echo "no such file: $f" >&2; exit 2; }
  tmp="$f.signing-$$"
  rm -f "$tmp"
  osslsigncode sign -pkcs12 "$pfx" -readpass "$pass_file" -h sha256 \
    -n "$name" -i "$url" -ts "$ts" -in "$f" -out "$tmp"
  report="$(osslsigncode verify "${verify_ca[@]}" -in "$tmp" 2>&1 || true)"
  if ! grep -q "Signature verification: ok" <<<"$report"; then
    rm -f "$tmp"
    echo "$report" >&2
    echo "verification failed: $f (left unsigned)" >&2
    exit 1
  fi
  mv "$tmp" "$f"
  grep -E "Subject:|Timestamp time|Signature verification" <<<"$report" || true
  echo "signed: $f"
done
