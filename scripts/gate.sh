#!/usr/bin/env bash
# Milestone quality gate. Usage: scripts/gate.sh [quick|full]
#   quick: fmt + clippy + tests          (run during development)
#   full:  quick + audit + secrets + SAST + trivy (run at milestones)
# Writes a summary to reports/gate-<timestamp>.md. Never exits 0 on a failed step.
set -uo pipefail
cd "$(dirname "$0")/.."
MODE="${1:-quick}"
# Local dev settings (DATABASE_URL for DB integration tests). Never printed.
if [ -f .env ]; then set -a; . ./.env; set +a; fi
mkdir -p reports
TS="$(date -u +%Y%m%dT%H%M%SZ)"
OUT="reports/gate-$TS.md"
FAIL=0

step() {
  local name="$1"; shift
  local log="reports/gate-$TS-${name}.log"
  if "$@" >"$log" 2>&1; then
    echo "| $name | PASS | \`$log\` |" >>"$OUT"
  else
    echo "| $name | **FAIL** | \`$log\` |" >>"$OUT"
    FAIL=1
  fi
}

{
  echo "# Quality gate $TS ($MODE)"
  echo
  echo "Commit: \`$(git rev-parse --short HEAD 2>/dev/null || echo uncommitted)\`  "
  echo "Dirty files: $(git status --porcelain 2>/dev/null | wc -l)"
  echo
  echo "| Step | Result | Log |"
  echo "|---|---|---|"
} >"$OUT"

# UI first: the Tauri crate embeds apps/desktop/dist at compile time.
if [ -f apps/desktop/package.json ]; then
  step ui-format pnpm -C apps/desktop format:check
  step ui-api-contract pnpm -C apps/desktop gen:api:check
  step ui-typecheck pnpm -C apps/desktop typecheck
  step ui-lint pnpm -C apps/desktop lint
  step ui-test pnpm -C apps/desktop test
  step ui-build pnpm -C apps/desktop build
fi

step fmt cargo fmt --all --check
step clippy cargo clippy --workspace --all-targets -- -D warnings
step test cargo test --workspace


if [ "$MODE" = "full" ]; then
  step cargo-audit cargo audit
  [ -f apps/desktop/package.json ] && step pnpm-audit pnpm -C apps/desktop audit --prod
  # `gitleaks git` only sees commits; `gitleaks dir` also catches uncommitted files.
  step gitleaks-tree gitleaks dir . --no-banner --redact --config .gitleaks.toml
  step gitleaks-history gitleaks git . --no-banner --redact --config .gitleaks.toml
  step semgrep semgrep scan --config p/rust --config p/typescript --config p/secrets --error --metrics=off --quiet .
  step trivy-fs trivy fs --scanners vuln,secret,misconfig --exit-code 1 --severity HIGH,CRITICAL --quiet .
fi

echo >>"$OUT"
if [ "$FAIL" -eq 0 ]; then echo "**Overall: PASS**" >>"$OUT"; else echo "**Overall: FAIL**" >>"$OUT"; fi
cat "$OUT"
exit "$FAIL"
