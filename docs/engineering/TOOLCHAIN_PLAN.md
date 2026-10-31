# Toolchain plan

Chosen to cover each language once, with no duplicate frameworks.

| Concern | Rust | TypeScript (UI) | SQL | Repo-wide |
|---|---|---|---|---|
| Format | rustfmt | Prettier | sqlfluff (Phase 1, optional) | — |
| Lint | clippy (`unwrap/expect/panic` denied, `unsafe` forbidden) | ESLint (typescript-eslint strict) | — | — |
| Types | rustc | `tsc --noEmit` strict | SQLx compile-time checked queries | — |
| Unit tests | cargo test, proptest | Vitest | — | — |
| Integration | cargo test against real PostgreSQL | — | migrations up/down test | — |
| Desktop E2E | — | WebdriverIO + @wdio/tauri-service (Xvfb) | — | — |
| UI in browser | — | Playwright (component/visual only) | — | — |
| Dependencies | cargo audit | pnpm audit | — | trivy fs |
| SAST | semgrep p/rust | semgrep p/typescript | — | semgrep p/secrets |
| Secrets | — | — | — | gitleaks (tree + history) |
| DAST | — | — | — | OWASP ZAP (docker) vs local server only |
| Debug | lldb, gdb, rust-gdb, `RUST_BACKTRACE`, tracing | WebKit inspector (dev builds only) | Postgres logs | correlation IDs |
| Desktop build | tauri-cli 2.12.4 | Vite | — | — |
| DB | sqlx-cli 0.9.0 | — | — | — |

Claude Code: plugins listed in `INSTALLATION_LOG.md`; agents in `.claude/agents/`; hooks in `.claude/settings.json`.
