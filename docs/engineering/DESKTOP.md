# Desktop client: build, install, update, troubleshoot

| Item | Value |
|---|---|
| Product name | Chuo-Sogo Procure |
| Bundle identifier | `net.chuo-sogo.procure` |
| Version source | `apps/desktop/src-tauri/tauri.conf.json` (`version`), kept equal to `package.json` and the crate version |
| Stack | Tauri 2.12 (Rust) + React 19 / TypeScript 5.9 (Vite 8), i18next |
| Icon source | `apps/desktop/src-tauri/icons/source.svg` → `cargo tauri icon` |

## Architecture of the client
- **Webview (TypeScript):** screens only. It calls seven named IPC commands (`build.rs` app manifest + `capabilities/default.json`); no shell, filesystem, HTTP or opener plugins are enabled.
- **Native layer (Rust):** holds the session token **in memory only**, adds a UUID `x-request-id` to every call, and only reaches `<server>/api/v1/...` (paths validated). The server address must be `https://`, except loopback `http://` for development.
- **CSP:** `default-src 'self'`, no remote scripts, no remote connections from the webview (`connect-src ipc:` only).
- **Local files:** `prefs.json` in the OS app-config directory (language and server address). No credentials and no procurement data are stored on the device.
- Closing the app or signing out discards the session; users sign in again on next start. "Stay signed in" via the OS keychain is a later option (see Known limitations).

## Build locally
```bash
pnpm -C apps/desktop install --frozen-lockfile
cd apps/desktop && cargo tauri dev                          # development window
cd apps/desktop && cargo tauri build --bundles deb,appimage # Linux installers
```
Outputs: `target/release/bundle/{deb,appimage}/`.

| OS | Formats | Where it can be built | Status |
|---|---|---|---|
| Linux | `.deb`, `.AppImage` | this server, CI `ubuntu-22.04` | Built on this server; see `docs/engineering/gates/` for the smoke-test record |
| Windows | `.msi`, NSIS `.exe` | CI `windows-latest` (manual `desktop-build` workflow) | Configured, **not yet built or tested** |
| macOS | `.dmg`, `.app` | CI `macos-latest` | Configured, **not yet built or tested** |

## Signing (founder decision, not done)
- **macOS:** Apple Developer Program (USD 99/yr), Developer ID Application certificate, notarisation (`APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` secrets). Unsigned apps are blocked by Gatekeeper on other Macs.
- **Windows:** code-signing certificate (OV/EV, or Azure Trusted Signing). Unsigned installers trigger SmartScreen warnings.
- **Linux:** optional GPG signature of `.deb`/AppImage.

## Updates (planned)
Tauri updater plugin with **signed** update manifests: generate a key pair with `cargo tauri signer generate`, keep the private key offline (founder custody), embed only the public key. Clients refuse unsigned or mis-signed updates. Not enabled until key custody and an HTTPS update host are decided.

## Install
- Debian/Ubuntu: `sudo apt install ./Chuo-Sogo\ Procure_0.1.0_amd64.deb`
- Other Linux: `chmod +x Chuo-Sogo*.AppImage && ./Chuo-Sogo*.AppImage`
- First start: open the server settings on the sign-in screen and enter the backend address (e.g. `https://procure.example.jp`).

## Troubleshooting
| Symptom | Check |
|---|---|
| "Cannot reach the server" | backend running? `curl https://<server>/health`; corporate proxy/firewall |
| "Insecure server address" | plain `http://` is only allowed for `127.0.0.1`/`localhost` |
| Blank window on Linux | install `libwebkit2gtk-4.1-0`; try `WEBKIT_DISABLE_DMABUF_RENDERER=1` |
| Tamil/Hindi shows boxes | install `fonts-noto-core` (Tamil, Devanagari) / `fonts-noto-cjk` (Japanese) |
| Need logs | start from a terminal with `RUST_LOG=procure_desktop_lib=debug,procure_ui=debug`; each line carries the request id that also appears in server logs |
