# 技術的ボトルネック分析 / Technical bottleneck analysis

2026-11-01、v1.0.0 以降の main で計測。トライアルでいただいたご指摘への回答です。

> 顧客の購入履歴だったり判断材料を集めて入れる必要がある。
> だからこそ後発から案件を取られにくいものになるが、それと同時に我々も受注が難しい。

## 結論

**最大のボトルネックは「最初のデータ」でした。** このシステムの価値（過去価格との比較、顧客の基準どおりの選定）は、
顧客の購買履歴と判断材料が入って初めて出ます。ところが v1.0.0 には、それを取り込む手段が
**ありませんでした**。履歴はこのシステムを通した取引からしか貯まらず、初日は空でした。
このため「入れれば強いが、入れるまでが重い（受注が難しい）」状態になっていました。

本日、この部分を実装しました（下記 §2）。残るボトルネックは、紙・FAX の読み取り（OCR）、
本物の AI モデル、取引からの自動蓄積、品目の名寄せ、日本国内ホスティングの順です（§3）。

## 1. ご指摘の構造

| | 内容 |
|---|---|
| 強み（参入障壁） | 履歴と基準が入るほど、他社に乗り換えると「前回価格」「基準」「実績」を失う |
| 弱み（受注の壁） | 初回に顧客がデータを集めて渡す手間がかかる。価値は導入後にしか見えない |
| 技術で解くべきこと | ① 既存の Excel/CSV/FAX をそのまま取り込めること ② 取り込みを当社スタッフが代行できること ③ 取り込んだ瞬間に価値（前回価格・価格差）が見えること |

## 2. 実装済み（本日、テスト済み）

| 機能 | 内容 | 状態 |
|---|---|---|
| 購買履歴の取り込み | Excel（.xlsx）・CSV・TSV、Shift_JIS/UTF-8。日本語・英語の見出しを自動判定（伝票日付・仕入先名・品名・型番・数量・単価（円）・金額(税抜) など）。表題行があっても見出し行を自動検出 | 実装・テスト済み |
| 日付・金額の正確な読み取り | 2024/1/5、2024年1月5日、令和6年1月5日、R6.1.5、H31/4/30、Excel のシリアル値。全角数字・全角型番。銭単位（¥0.85）まで整数で正確に保存。金額÷数量から単価を算出した行は印付き | 実装・テスト済み |
| 確認してから保存 | まずプレビュー（何も保存しない）。列の割り当てを画面で修正可能。読めない行は推測せず「日付が読めない」等の理由付きで除外 | 実装・テスト済み |
| 取り消し | 取り込み単位で削除（誤ったファイル・列の場合）。監査ログに記録 | 実装・テスト済み |
| 判断材料パネル | 依頼の各品目に「前回価格・仕入先・日付」「価格帯」「最安の仕入先」「購買回数」を表示。型番（担当者が確認した型番を優先）→品名の順で照合 | 実装・テスト済み |
| 購買基準 | 優先順位（価格・納期・品質・既存取引先・国内品）、優先/使わない仕入先、必要な認証、最長納期、価格上昇の注意ライン、その他ルール。手配担当者に表示 | 実装・テスト済み（注意ラインは表示のみ。見積比較での自動判定は未実装） |
| 当社スタッフによる代行 | 顧客に割り当てられた当社スタッフが、顧客の代わりにデータを取り込める（受注の壁を下げる「導入代行」） | 実装・テスト済み |
| 権限と分離 | 依頼者は過去価格を見られない。経理は閲覧のみ。他社からは存在も見えない（PostgreSQL 行レベルセキュリティ） | 実装・テスト済み |

## 3. 残るボトルネック（影響の大きい順）

| # | ボトルネック | 現状 | 影響 | 解決策 | 目安 |
|---|---|---|---|---|---|
| B1 | 紙・FAX の購買記録 | スキャン PDF は文字が無く「読み取れない」になる。サーバーに OCR が無い | 中小製造業の約37%が FAX 中心。履歴の多くが紙 | 当社サーバー内で OCR（tesseract 日本語、外部送信なし）→ 表形式に変換 → 同じプレビュー画面で人が確認 | 3〜5日 |
| B2 | AI がモック | 品目抽出・掲載支援は仮のモデル | 「AI で楽になる」をまだ実演できない | D5：国内・ローカルモデルか API かを決定し、実データで精度を測定 | 決定後 1〜2週 |
| B3 | 取引からの自動蓄積 | 当システムで発注・検収した結果は履歴に自動では入らない | 使うほど賢くなる循環がまだ無い | 検収・照合完了時に購買履歴へ自動登録（出典「当システム」） | 2〜3日 |
| B4 | 品目の名寄せ | 型番（記号・全角を正規化）か、品名の完全一致のみ | 「6204ZZ」と「6204-2Z」、表記ゆれは別物扱い | 別名表 → 担当者が確定した対応を学習 → 将来は類似検索 | 1週〜 |
| B5 | 見積と前回価格の自動比較 | 判断材料パネルで人が見比べる | 価格上昇を見落とす可能性 | 見積比較に「前回比 +x%」と注意ライン超過の印を追加 | 2日 |
| B6 | データの置き場所への不安 | サーバーは国外・単一、バックアップは同一機 | 購買履歴を預ける心理的障壁（受注の壁そのもの） | D8：国内ホスティング、別拠点の暗号化バックアップ、NDA・取扱説明書 | 決定後 1週 |
| B7 | 配布・信頼 | 署名なしインストーラー、Mac は GitHub 課金停止で再ビルド不可 | 初回インストール時の警告 | D9：コード署名、課金の解消 | 決定後 数日 |

B1・B3・B5 は決定待ちが無く、すぐ着手できます。B2・B6・B7 は創業者の決定（費用・契約）が必要です。

## 4. 受注の壁を下げる運用の提案

1. **「無料の価格診断」から入る。** 過去1年分の購買記録（Excel/CSV/FAX）を預かり、当社スタッフが取り込み、
   品目ごとの価格帯と「同じ品目を高く買った例」を一覧で返す。顧客の手間はファイルを渡すだけ。
   取り込み・照合は今回の機能でそのままできる（一覧レポートの出力は未実装・2日程度）。
2. **導入代行を標準に。** 顧客に作業をさせない。担当スタッフを割り当てて代行（機能は実装済み）。
3. **基準は面談で一緒に書く。** 購買基準の画面を、初回面談でその場で埋める。

## 5. 検証

- ドメイン単体テスト：見出し判定・日付（和暦・Excel シリアル）・金額の正確さ・名寄せキー・価格集計（9件）
- データベース結合テスト：Shift_JIS の ERP 出力の取り込み、Excel ブック（表紙シート付き、日付セル、銭単位の単価）、プレビュー、取り消し、権限・他社分離、購買基準（5件）
- 画面テスト：プレビュー→列修正→再確認→取り込み、閲覧のみの権限、判断材料パネル（6件）
- 品質ゲート：`scripts/gate.sh full` の結果は reports/ に保存

未検証：実際の顧客の Excel・基幹システム出力での試験はまだ（テスト用に作成したファイルのみ）。
50,000 行の取り込み速度は未計測。

---

## English summary

The reviewers' point: the service only becomes valuable (and hard for a competitor to displace) once the
customer's purchase history and decision criteria are in it, and that same requirement makes the first
sale hard. Technically, the bottleneck was that v1.0.0 had **no way to bring that data in**: history could
only accumulate from deals run through the system, so every customer started empty.

Built today, with tests: Excel/CSV purchase-history import (Japanese ERP exports, Shift_JIS, era dates,
sub-yen prices, preview, column mapping, rows rejected with reasons rather than guessed, whole-import
withdrawal, audit), a per-line price reference on every request (last price, supplier, range, cheapest
source), the customer's written buying criteria, and import by the assigned Chuo-Sogo staff so the
customer does not have to do the work.

Remaining bottlenecks, in order: OCR for paper/FAX records (B1), a real AI model instead of the mock (B2),
feeding completed orders back into history (B3), item-name matching beyond exact part numbers (B4),
automatic quote-vs-last-price flags (B5), Japan hosting and off-site backups (B6), and code signing / the
GitHub billing block (B7). B1, B3 and B5 need no decision and can start now.
