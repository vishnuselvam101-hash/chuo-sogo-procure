# Security findings

| ID | Date | Title | Severity | ASVS | Status | Evidence |
|---|---|---|---|---|---|---|
| SF-001 | 2026-10-31 | Gate missed secrets in uncommitted files (`gitleaks detect` history-only) | Medium (tooling) | — | Fixed | canary run; `scripts/gate.sh` now runs `gitleaks dir` + `gitleaks git` |

Baseline scan 2026-10-31 on sample crate: cargo audit, gitleaks, semgrep (p/rust, p/typescript, p/secrets), trivy fs → 0 findings. Not meaningful yet (≈150 lines of code, no dependencies, no server).
