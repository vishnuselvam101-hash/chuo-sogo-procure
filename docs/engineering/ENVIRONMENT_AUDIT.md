# Environment audit — 2026-10-31

| Item | Finding |
|---|---|
| Host | Hostinger VPS, headless (no display), Ubuntu 26.04.1 LTS, kernel 7.0.0-38 |
| CPU / RAM | 8 cores / 31 GiB (≈9 GiB available; other VNX services running) |
| Disk | `/` 387 G, 136 G free; `/tmp` 16 G tmpfs |
| Rust | 1.95.0 via isolated rustup `/opt/rust` (wrappers in `/usr/local/bin`); system 1.93 also present |
| Node / pnpm | v24.21.0 / 12.15.0 |
| Python | 3.14.4 (not used in v1) |
| PostgreSQL | 18.6, service active |
| Docker | 29.1.3, active |
| Git | 2.53.0 |
| Repository | None existed. Created `/root/procurement-platform` (git, branch `main`, no commits yet) |
| Existing CI / tests / lint | None (greenfield) |
| Claude Code plugins before | claude-security, code-review, code-simplifier, clangd-lsp, document-skills, cloudflare, claude-obsidian, claude-md-management (user scope) |

## Compatibility notes
- Headless server: Tauri dev window needs Xvfb; WebdriverIO desktop tests use `WebKitWebDriver` under Xvfb.
- Only Linux installers can be built and smoke-tested here. macOS needs a Mac (signing/notarisation). Windows MSI needs Windows or CI.
- Memory is shared with other workloads; parallel Rust builds are capped (`CARGO_BUILD_JOBS` may need lowering).
