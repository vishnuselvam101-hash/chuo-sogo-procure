# Quality gates

`scripts/gate.sh quick` — during development: rustfmt check, clippy `-D warnings`, cargo test (+ UI typecheck/lint/test once the app exists).
`scripts/gate.sh full` — at milestones: quick + cargo audit + gitleaks (tree and history) + semgrep (p/rust, p/typescript, p/secrets, metrics off) + trivy fs (HIGH/CRITICAL).

Milestone done only when:
1. `gate.sh full` PASS (report in `reports/`, summary copied to `docs/engineering/gates/<milestone>.md`)
2. Acceptance criteria verified end-to-end (qa-engineer)
3. Security-sensitive diff reviewed (cybersecurity-expert); no open Critical/High
4. Independent code review (code-reviewer); confident defects fixed and re-tested
5. Docs + known limitations updated
6. Then commit (author `vishnuselvam101-hash`, no AI trailers)

## Gate verification (canary, 2026-10-31)
A copy of the repo with five planted defects (fmt violation, `unwrap`, failing assertion, fake AWS key in code, fake GitHub token in uncommitted file) → fmt, clippy, test, semgrep, trivy, gitleaks-tree all FAIL. Clean repo → all PASS.
