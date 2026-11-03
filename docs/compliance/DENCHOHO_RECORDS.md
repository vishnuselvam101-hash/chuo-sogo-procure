# Electronic transaction records (電子帳簿保存法) in Chuo-Sogo Procure

**Draft. Not legal or tax advice.** The factory keeps the records; it must confirm this with its
tax adviser (税理士) before relying on it.

## What the law asks (NTA pamphlet, 2024)

Quotations, orders, invoices and contracts exchanged electronically (email bodies, attached
PDFs, spreadsheets, FAX received as a file) must be kept electronically, and the company must
meet two requirements:

1. **Tamper prevention**, by one of: a time stamp; a system in which records cannot be changed
   or deleted, or every change is kept; or written internal rules on correction and deletion
   (事務処理規程).
2. **Search** by **transaction date, amount and counterparty**, with ranges and
   combinations. A company that hands the data over when the tax office asks for a download
   may skip ranges and combinations. A ≥¥10bn factory gets no exemption.

Sources: https://www.nta.go.jp/publication/pamph/sonota/0021011-068.pdf,
https://www.mof.go.jp/public_relations/finance/202408/202408c.html

## How the product meets them

| Requirement | In the product |
|---|---|
| Keep what the supplier sent | Every reply file read for a quotation (email `.eml` with its attachments, PDF, Excel/CSV, FAX scan) is stored as received in `supplier_documents` with its SHA-256 (migration 0019). |
| Cannot be changed or deleted | The server's database role may only add and read supplier documents; changes and deletions are refused by the database. Quotations are never edited: a revision supersedes the old one, which stays. Every action is in the hash-chained audit trail. |
| Date / amount / counterparty search, ranges, combinations | **Records** page (`GET /orgs/{org}/records`): quotations, orders, supplier invoices and Chuo-Sogo's fee invoices, by date range (factory time zone), amount range, counterparty name and kind, all combinable. |
| Show the file | Each quotation links to its file, which can be saved with its SHA-256 for comparison. |

Not covered yet: emails that never became a quotation (for example a refusal) are kept only if
staff upload them; orders the factory sends from its own system are kept there.

## Template: 電子取引データの訂正及び削除の防止に関する事務処理規程 (outline)

The factory adapts this to its own organisation; the NTA publishes an official sample.

1. **目的** — 電子取引データの真実性を確保するため、訂正・削除の防止について定める。
2. **適用範囲** — 当社が電子取引で授受する見積書・注文書・請求書・契約書等。購買業務代行
   （中央総合株式会社）のシステム上で授受したものを含む。
3. **管理責任者** — 経理部長（例）。購買部長を処理責任者とする。
4. **訂正・削除の原則禁止** — 保存した電子取引データは訂正・削除しない。
5. **やむを得ない訂正・削除** — 処理責任者の承認を得て行い、訂正前後の内容・理由・日時・
   担当者を記録する（本システムでは改訂見積は新しい記録として追加され、旧記録は残る）。
6. **保存期間** — 法定期間（原則7年、欠損金がある事業年度は10年）保存する。
7. **検索** — 取引年月日・取引金額・取引先で検索できる状態で保存する（本システムの
   取引記録画面、または税務職員の求めに応じたダウンロード）。
8. **附則** — 施行日。
