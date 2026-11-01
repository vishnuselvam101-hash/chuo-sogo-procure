# Quality gate — Phase A (admin console), 2026-11-01

## Scope
Server (f97d81c): `/api/admin/v1` namespace, admin-audience sessions, TOTP second factor
(RFC 6238, secrets AES-256-GCM-sealed with `PROCURE_SECRET_KEY`), network allow-list,
per-organisation rules (`org_settings`), organisation/account management, health.
Desktop (this commit): **Chuo-Sogo Procure Admin**, a separate app built from the same crate
and frontend (`--features admin`, `tauri.admin.conf.json`, Vite `--mode admin`):
- native layer reaches only `/api/admin/v1/`; the step token between password and code stays
  in Rust like the session token; no file download command; own capability (`admin.json`),
  own identifier (`net.chuo-sogo.procure.admin`), own preferences;
- screens: sign-in (password → authenticator enrolment or code), overview (health),
  organisations (create, filter), organisation detail (members, rules, staff assignment,
  audit log with chain check, name/language/time zone, suspend/reactivate), accounts
  (search, deactivate/reactivate, unlock, reset authenticator; never one's own account),
  settings (language). Dangerous actions need a second explicit click. ja/en/ta/hi.
- the business build contains no admin code and vice versa (checked in the built bundles).

## Automated
`scripts/gate.sh full` — all 17 steps **PASS** (new: clippy and tests of the admin build).

| Suite | Count | Result |
|---|---|---|
| Server: admin integration | 3 | pass |
| Native layer, business build / admin build (prefix isolation, capability isolation, TOTP code format) | 11 / 10 | pass |
| Desktop UI (Vitest), incl. admin sign-in steps, expired step, failed console load, admin-only API paths | 28 | pass |
| Admin E2E (release binary, real server, TOTP computed like an authenticator app): enrol → health → rules saved and reloaded → staff assigned → audit chain + rule change logged → suspend/reactivate → deactivate/reactivate account → own account protected → wrong code refused → second sign-in | 15 checks | **15/15 pass** |

## Review
Local independent review of the client: no token leakage, prefix bypass or cross-build command
exposure found. Fixed: SF-042 (sign-in stuck with a live session when the console failed to
load), SF-043 (step token kept after "back to sign-in"); added the capability isolation test.

## Not done
The server is not yet installed as a service on this machine (needs the founder to run
`scripts/install-server.sh`). Admin installers are built by `desktop-build` with `app: admin`
(not run yet). zh-Hans/ko fall back to English for the admin screens (as for the rest of the app).
Re-authentication before dangerous actions (plan section 3) is not implemented yet: the
30-minute idle limit and the two-click confirmation apply.
