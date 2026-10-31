# Installation log — 2026-10-31

## System and toolchain
| Tool | Version | Source | Verified by |
|---|---|---|---|
| libwebkit2gtk-4.1-dev | 2.52.6 | apt | dpkg-query |
| webkitgtk-webdriver (`WebKitWebDriver`) | 2.52.6 | apt | `which WebKitWebDriver` |
| xvfb, patchelf, libxdo-dev, libayatana-appindicator3-dev, librsvg2-dev, libssl-dev, build-essential, file, pkg-config | Ubuntu 26.04 | apt | dpkg-query |
| lldb | 21.1.8 | apt | `lldb --version` |
| rustfmt / rust-analyzer | 1.9.0 / 1.95.0 | rustup component (1.95) + wrappers in `/usr/local/bin` | `--version` |
| tauri-cli | 2.12.4 | cargo install --locked | `cargo tauri --version` |
| cargo-audit | 0.22.2 | cargo install --locked | `cargo audit --version` |
| sqlx-cli | 0.9.0 (rustls, postgres) | cargo install --locked | `sqlx --version` |
| semgrep CE | 1.183.0 | pipx | `semgrep --version`; run with `--metrics=off` |
| trivy | 0.76.1 | GitHub release .deb, SHA-256 checked | `trivy --version` |
| gitleaks | 8.30.1 | pre-installed | `gitleaks version` |
| OWASP ZAP | stable (3.75 GB image) | `ghcr.io/zaproxy/zaproxy:stable` | `docker image ls` |

Issue during install: first apt run failed silently because `webkit2gtk-driver` has no candidate on 26.04 (package is `webkitgtk-webdriver`). Fixed and re-run; all packages verified installed afterwards.

## Claude Code plugins (project scope, all show "✔ enabled" in `claude plugin list`)
| Plugin | Source | Version | Purpose | External/paid dependency |
|---|---|---|---|---|
| superpowers | anthropics/claude-plugins-official | 7.0.0 | TDD, debugging, planning skills | none |
| security-guidance | anthropics/claude-plugins-official | 2.0.17 | edit-time security warnings | none |
| playwright | anthropics/claude-plugins-official (Microsoft MCP) | a7ba5b93db0c | browser automation | none (downloads browsers on first use) |
| differential-review | trailofbits/skills | 1.1.4 | security review of diffs | none |
| rust-review | trailofbits/skills | 1.1.2 | Rust security review agents | none |
| insecure-defaults | trailofbits/skills | 2.0.3 | hardcoded creds, fail-open config | none |
| sharp-edges | trailofbits/skills | 1.1.3 | footgun API design | none |
| static-analysis | trailofbits/skills | 1.5.1 | Semgrep/CodeQL/SARIF workflows | CodeQL optional, not installed |
| supply-chain-risk-auditor | trailofbits/skills | 2.0.4 | npm/crate dependency risk | none |
| property-based-testing | trailofbits/skills | 1.2.2 | proptest guidance | none |
| code-review, claude-security | already installed (user scope) | 1.0.1 / 0.11.0 | review, deep vuln scan | none |

Licences (checked 2026-10-31): trailofbits/skills is CC-BY-SA-4.0; anthropics/claude-plugins-official repository is Apache-2.0 (individual third-party plugins such as playwright carry their own upstream licences). None are redistributed with this product.
