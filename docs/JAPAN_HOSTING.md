# 国内ホスティングとデータの置き場所（B6 / D8）

顧客が購買履歴を預ける心理的な壁を下げるための準備です。**契約・購入は創業者の判断（D8）**で、
この文書と下記のスクリプトは「契約したら設定を入れるだけ」の状態にするためのものです。

## 1. 今できていること（2026-11-02、テスト済み）

| 項目 | 内容 |
|---|---|
| 毎晩のバックアップ | データベース＋保存ファイル、SHA-256 付き、14 世代（`procure-backup.timer`、03:30 JST） |
| 復元訓練 | `scripts/restore-drill.sh`（2026-11-01 本番で成功） |
| **別拠点への暗号化コピー（新規）** | `scripts/procure-offsite.sh`：最新のバックアップを検査 → サーバー上で創業者の公開鍵で暗号化（GnuPG）→ rclone で国内オブジェクトストレージ等へ送信 → 読み戻して SHA-256 照合 → 30 世代保持。秘密鍵はサーバーに置かないため、保管業者もサーバーを盗んだ人も中身を読めない。`deploy/procure-offsite.{service,timer}`（04:30 JST）。**設定するまで何もしない** |
| 別拠点からの復元（新規） | `scripts/procure-offsite-restore.sh <保存先> latest <出力先>`（秘密鍵が必要）→ 復元訓練へ |

テスト結果（ローカルの保存先で実施）：暗号化して送信・読み戻し照合 OK、同じものは二重に送らない、
公開鍵だけでは復号できない、秘密鍵で復元したファイルは元と完全一致、チェックサムが合わない
バックアップは送らない（終了コード 1、何も送信しない）。

## 2. 有効にする手順（契約後）

1. 創業者の手元（オフライン）で鍵を作る：`gpg --quick-gen-key "Chuo-Sogo Procure backup" rsa4096 encr never`。
   秘密鍵は紙・USB 等で2か所に保管。公開鍵だけをサーバーへ：
   `gpg --export <指紋> | sudo -u procure GNUPGHOME=/etc/procure/gnupg gpg --import`
2. 保存先を rclone に登録（認証情報は `/etc/procure/rclone.conf`、所有者 procure・権限 600）。
3. `/etc/procure/procure.env` に追記：
   `PROCURE_OFFSITE_TARGET=jp-backup:procure-backups`、`PROCURE_OFFSITE_GPG_RECIPIENT=<指紋>`、
   `RCLONE_CONFIG=/etc/procure/rclone.conf`
4. `scripts/install-server.sh`（スクリプトとユニットを配置）→ `systemctl enable --now procure-offsite.timer`
5. 初回は手動で：`systemctl start procure-offsite.service && journalctl -u procure-offsite -n 20`
6. 月1回：別のマシンで `procure-offsite-restore.sh` → `restore-drill.sh` を実施し記録。

## 3. 国内ホスティングの選択肢（**価格・仕様は創業者が公式ページで要確認**）

| 候補 | 種類 | 向いている点 | 確認すること |
|---|---|---|---|
| さくらのクラウド / VPS（石狩・東京・大阪） | 国内事業者の VPS・クラウド、オブジェクトストレージあり | 国内企業への説明がしやすい、ISMAP 登録サービスあり | プラン・料金、オブジェクトストレージの S3 互換性 |
| ConoHa VPS（GMO） | 国内 VPS | 安価に始めやすい | バックアップ・SLA |
| IIJ GIO | 国内法人向けクラウド | 大手製造業の情報システム部門に通りやすい | 最低契約・料金 |
| AWS 東京 / 大阪、Google Cloud 東京 | 外資の国内リージョン | 冗長化・マネージド DB・監視が揃う | データの所在（国内リージョン指定）、料金、契約主体 |

推奨（暫定）：最初の顧客までは「現在のサーバー＋国内オブジェクトストレージへの暗号化コピー」で開始し、
有償顧客が決まった時点でアプリを国内 VPS / クラウドへ移す。移行は下の手順で 1 日以内。

## 4. 移行手順（このサーバー → 国内サーバー）

1. 新サーバー：Ubuntu LTS、PostgreSQL 16、`fonts-noto-cjk poppler-utils ffmpeg tesseract-ocr tesseract-ocr-jpn`。
2. リポジトリを配置し `scripts/db-setup.sh`、リリースビルド、`scripts/install-server.sh`。
3. 旧サーバーで最新バックアップ → 新サーバーで `restore-drill.sh` と同じ手順で本番 DB に復元、`procure-server verify`。
4. TLS 付きの公開（独自ドメイン、例：procure.chuo-sogo.net）、DNS 切替、デスクトップアプリの接続先を更新して再配布。
5. 旧サーバーを読み取り専用で 2 週間保持 → 消去証明を記録。

---

## データ取扱説明書（顧客向け・**草案：法務確認前**）

**中央綜合株式会社 調達サービス：お預かりするデータの取り扱い**

1. **お預かりするもの**：購買依頼、見積、注文・納品・請求の記録、貴社が提供する購買記録（Excel・CSV・PDF・スキャン・FAX）、購買基準。
2. **保存場所**：当社が管理するサーバー（現在：〔所在地を記入〕、国内移行予定：〔事業者・地域を記入〕）。バックアップは毎晩作成し、別拠点に暗号化して保管します。
3. **他社との分離**：貴社のデータはデータベースの行レベルセキュリティで他社から分離され、他社の利用者からは存在も見えません。
4. **アクセスできる人**：貴社が招待した利用者と、貴社を担当する当社スタッフのみ。当社スタッフのアクセスは担当割り当てに基づき、すべて監査ログに記録されます。
5. **文字認識・AI**：スキャン・FAX の読み取り（OCR）は当社サーバー内で行い、外部に送信しません。AI 機能の外部サービス利用は、貴社の同意なく行いません。AI が発注・承認を行うことはありません。
6. **通信と保管の暗号化**：通信は TLS で暗号化します。別拠点のバックアップは当社の鍵で暗号化され、保管業者は内容を読めません。
7. **削除**：契約終了時または貴社のご依頼により、データを削除し、削除完了を書面でご報告します（バックアップは保持期間〔30日〕経過後に消去）。
8. **事故時の連絡**：漏えい等のおそれがある場合、判明後〔72 時間〕以内にご連絡します。
9. **お問い合わせ**：〔窓口を記入〕

---

## English summary

Preparation for D8 (no purchase made). Implemented and tested: `scripts/procure-offsite.sh` encrypts the
newest nightly backup on the server to the founder's offline public key (GnuPG), uploads it with rclone
(Japan object storage, SFTP or a path), reads it back and checks SHA-256, skips copies already uploaded,
refuses a backup whose checksums fail, and keeps 30 copies; `procure-offsite-restore.sh` fetches, verifies
and decrypts with the private key. Tested against a local target: upload + verify, idempotence, the server's
keyring cannot decrypt, restore is byte-identical, tampered backup refused. It does nothing until
`PROCURE_OFFSITE_TARGET` and `PROCURE_OFFSITE_GPG_RECIPIENT` are set; the timer is installed but not
enabled. Hosting options in Japan are listed with prices to be confirmed by the founder, a one-day migration
runbook is included, and a customer data-handling statement is drafted for legal review.
