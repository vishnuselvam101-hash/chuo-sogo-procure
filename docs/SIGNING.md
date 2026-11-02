# 署名と配布の信頼性（B7 / D9）

署名の無いインストーラーは、Windows では SmartScreen の「WindowsによってPCが保護されました」、
Mac では「開発元を確認できません」と表示されます。顧客の情報システム部門ではこれだけで導入が止まることがあります。
**証明書・開発者登録の購入は創業者の判断（D9）**。ここでは、購入後に設定を入れるだけで署名できる状態を用意しました。

## 1. 購入が必要なもの（**価格は公式ページで要確認**）

| 対象 | 必要なもの | 解消される表示 | 備考 |
|---|---|---|---|
| Windows | コード署名証明書（OV または EV）。発行元例：DigiCert、Sectigo、GlobalSign（国内代理店あり） | SmartScreen 警告（EV は即時、OV は評判の蓄積後） | 2023年以降、鍵はハードウェアトークンかクラウド HSM で保管が必須。法人の登記確認あり（中央綜合株式会社名義） |
| Mac | Apple Developer Program（法人、D-U-N-S 番号が必要） | Gatekeeper 警告（公証 = notarization） | Developer ID Application 証明書で署名 → `notarytool` で公証 |
| Android | Google Play デベロッパー登録（ストア公開する場合） | 「提供元不明のアプリ」 | 現在の APK は使い捨てのテスト鍵。本番鍵は創業者が保管 |
| CI | GitHub Actions の支払い設定の解消 | Mac の再ビルドができない問題 | 現在は課金上限で停止中。Windows/Linux/Android はこのサーバーで作成可能 |

## 2. 用意したもの（2026-11-02）

- `scripts/release/sign-windows.sh <exe...>`：osslsigncode で Authenticode 署名（SHA-256）、
  RFC 3161 タイムスタンプ付き（証明書の期限後も署名が有効）、署名後に検証し、検証に失敗したら未署名のまま残す。
  設定（`PROCURE_SIGN_PFX`、`PROCURE_SIGN_PASSWORD_FILE`）が無ければ何もせず不足項目を表示して終了。
  - **テスト結果**：使い捨ての自己署名証明書で、実際のトライアル用 Setup.exe（コピー）に署名 →
    DigiCert のタイムスタンプ取得 → `Signature verification: ok`。自己署名なので Windows では信頼されない（想定どおり）。
    なお、タイムスタンプ取得時にファイルのハッシュ値だけが DigiCert の公開タイムスタンプサーバーに送られます。
  - EV 証明書（トークン / クラウド HSM）の場合：トークンを挿した Windows 機で `signtool sign /tr <TSA> /td sha256 /fd sha256 /a <exe>`、
    またはベンダーのクラウド署名（例：DigiCert KeyLocker、SSL.com eSigner）を使います。Tauri の `bundle.windows.signCommand` に設定可能。

## 3. 購入後の手順

### Windows（OV、PFX で受け取る場合）
1. PFX と パスワードファイルを `/etc/procure/signing/`（root のみ、権限 600）に置く。
2. `scripts/demo/build-trial-local.sh` でビルド後：
   `PROCURE_SIGN_PFX=/etc/procure/signing/cs.pfx PROCURE_SIGN_PASSWORD_FILE=/etc/procure/signing/pass scripts/release/sign-windows.sh target/trial-out/desk-*/**.exe`
3. 署名後に `make-trial-package.sh` → `publish-downloads.sh`（SHA-256 が変わるのでダウンロードページの値も更新）。

### Mac
1. Apple Developer Program に法人登録 → Developer ID Application 証明書、App 用パスワード。
2. GitHub の `desktop-build` に `APPLE_CERTIFICATE`、`APPLE_CERTIFICATE_PASSWORD`、`APPLE_ID`、`APPLE_PASSWORD`、`APPLE_TEAM_ID` を設定（Tauri が署名と公証を実行）。
3. GitHub Actions の支払い設定を解消してから実行。

### Android
1. 創業者の手元で本番鍵を作成：`keytool -genkeypair -v -keystore chuo-sogo-release.jks -alias release -keyalg RSA -keysize 4096 -validity 10000`。
   紛失するとアプリを更新できなくなるため、2か所に保管。
2. ビルド時に `PROCURE_ANDROID_KEYSTORE`、`PROCURE_ANDROID_KEY_ALIAS`、パスワードファイルを渡して `apksigner sign` → `apksigner verify`。

### 自動更新（Tauri updater）
- 更新用の署名鍵は創業者の手元で `pnpm tauri signer generate -w ~/.tauri/chuo-sogo.key` で作成（秘密鍵はリポジトリに入れない）。
  公開鍵を `tauri.conf.json` の `plugins.updater.pubkey` に設定し、更新ファイルの置き場（HTTPS）を決めてから有効化。**まだ有効にしていません。**

---

## English summary

What to buy (founder, D9): a Windows code-signing certificate (OV or EV; keys must live on a token/HSM),
the Apple Developer Program (company, D-U-N-S) for signing + notarisation, optionally Google Play, and the
GitHub Actions billing fix for Mac builds. Ready now: `scripts/release/sign-windows.sh` signs with
osslsigncode (SHA-256 Authenticode + RFC 3161 timestamp), verifies, and leaves the file unsigned if
verification fails; it refuses to run without a certificate. Tested with a throwaway self-signed certificate
on a copy of the real trial Setup.exe: signature and DigiCert timestamp verified (self-signed is not trusted
by Windows, as expected; only a hash was sent to the public timestamp server). Mac/Android/updater steps
are documented; no real keys were generated.
