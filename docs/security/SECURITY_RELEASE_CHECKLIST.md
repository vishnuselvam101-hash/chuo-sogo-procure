# Security release checklist

- [ ] `scripts/gate.sh full` PASS on the release commit
- [ ] No open Critical/High in SECURITY_FINDINGS.md
- [ ] All 10 critical scenarios pass
- [ ] ASVS 5.0.0 L2 checklist reviewed, gaps recorded
- [ ] ZAP baseline/API scan vs local release build: no unresolved High
- [ ] Tauri: capabilities minimal, CSP strict, devtools off, no private macOS API, updater pubkey embedded and signature verified
- [ ] gitleaks + `strings` scan of every installer/binary: no secrets
- [ ] Dependencies: cargo audit, pnpm audit, trivy clean or accepted with reason
- [ ] Logs redact tokens/passwords/file contents
- [ ] Backup + restore drill done on this release's schema
- [ ] Founder approval for signing, publishing and deployment
