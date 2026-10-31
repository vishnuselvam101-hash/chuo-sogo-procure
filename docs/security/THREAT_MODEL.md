# Threat model (v0, design stage)

## Assets
Customer procurement data (requests, specs, drawings), supplier quotations and prices (commercially sensitive, per-tenant), approval decisions, PO/invoice records, audit log, user credentials/sessions, AI provider keys, update-signing key.

## Actors
Customer users (7 roles across orgs), Chuo-Sogo operators/admins, suppliers (send documents/emails = untrusted), external attacker, malicious insider, compromised dependency, the AI model itself (untrusted output).

## Trust boundaries
1. Webview ↔ Tauri Rust layer (IPC) 2. Desktop ↔ API (network) 3. API ↔ DB (tenant context) 4. API ↔ AI provider 5. Uploaded files/emails ↔ parsers/AI 6. Build/CI ↔ release artefacts/updater.

## Top threats → controls
| ID | Threat | Control | Test |
|---|---|---|---|
| T1 | Org A reads org B data (IDOR, missing filter) | TenantCtx in repos + RLS | isolation suite on every endpoint |
| T2 | Operator over-reach across tenants | service_assignments, audited access | operator without assignment → 404 |
| T3 | Requester approves own purchase / exceeds limit | policy engine in domain, server-only | approval tests |
| T4 | AI approves, issues PO or substitutes product | AI writes only `ai_suggestions`; no tool access to workflow | injection corpus + "approve" outputs inert |
| T5 | Prompt injection in supplier PDF/email | fenced untrusted text, schema-validated output, per-tenant context only | hostile corpus |
| T6 | Cross-tenant AI context leak | context builder takes one TenantCtx; no shared caches keyed without org | test with two orgs' docs |
| T7 | Malicious upload (polyglot, zip bomb, huge) | size limits, magic-byte allowlist, quarantine until scanned, no server-side execution | upload tests |
| T8 | Session theft | opaque tokens hashed at rest, keychain on desktop, short idle timeout, revocation | session tests |
| T9 | Credential stuffing | argon2id, per-account + per-IP throttling | rate-limit tests |
| T10 | Audit tampering | append-only grants, hash chain | chain verification test |
| T11 | Quotation/invoice tampering after approval | approved snapshot hashed into approval record; edits create new versions | versioning tests |
| T12 | Malicious update / supply chain | signed updater manifests, cargo audit/pnpm audit/trivy, lockfiles, pinned CI actions | gate |
| T13 | Webview compromise → native access | minimal Tauri capabilities, strict CSP, no shell/fs plugins, token never in webview | capability review |
| T14 | Secrets in desktop build | no secrets in client; config fetched after login | gitleaks + binary string scan |
| T15 | Sensitive data in logs | redaction middleware | log tests |
