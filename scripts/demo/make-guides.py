#!/usr/bin/env python3
"""Trial guides in Japanese and English from one source, as HTML and A4 PDF.

    python3 scripts/demo/make-guides.py <out-dir>

Writes guide-ja.html, guide-en.html, ChuoSogo-Trial-Guide-ja.pdf and ChuoSogo-Trial-Guide-en.pdf.
PDFs are printed with headless Chromium (CHROME, default: Playwright's Chromium). No passwords
appear in the guides; the shared trial password is handed over separately.
"""
import glob
import html
import os
import subprocess
import sys

DEMO = "https://vnx-hostinger.taild091df.ts.net:8443"
PAGE = "https://vsnexustechnologies.com/trial/procure"
VERSION = "1.0.0"

T = {
    "ja": {
        "title": "中央総合 調達プラットフォーム 試用ガイド",
        "sub": f"バージョン {VERSION}",
        "lead": "見積依頼から承認・発注・請求書までを一つのアプリで管理する業務用ソフトです。インストールするだけで試用サーバーにつながり、接続先の設定は不要です。表示されるデータはすべて架空のデモデータです。",
        "get": "1. ダウンロード",
        "get_body": f"次のページを開くと、お使いの端末（Windows・Mac・Android など）に合ったファイルが自動で表示されます。",
        "install": "2. インストール",
        "signin": "3. 始め方（パスワード不要）",
        "signin_body": "アプリを開くと役割のボタンが並びます。押すだけで始められ、パスワードは要りません。役割を変えるときは、メニューの「サインアウト」で最初の画面に戻ります。画面の言語メニューで日本語 / English を切り替えられます。",
        "try": "4. 試してほしいこと（約30分）",
        "notes": "5. ご注意",
        "role": "役割", "email": "メールアドレス", "app": "アプリ",
        "device": "端末", "req": "動作環境",
        "unsigned": "警告が出るのは、試用版にまだ電子署名をしていないためです。正式版では署名します。",
        "server": "試用サーバー",
    },
    "en": {
        "title": "Chuo-Sogo Procurement Platform — Trial Guide",
        "sub": f"Version {VERSION}",
        "lead": "Business software for procurement: requests, approvals, orders and invoices in one app. Install it and it connects to the trial server by itself; there is nothing to set up. All data shown is fictional demo data.",
        "get": "1. Download",
        "get_body": "Open the page below. It shows the right file for your device (Windows, Mac, Android and so on) automatically.",
        "install": "2. Install",
        "signin": "3. Getting started (no password)",
        "signin_body": "When the app opens it shows one button per role. Press one to start; no password is needed. To change role, use “Sign out” in the menu to return to the first screen. The language menu in the app switches between 日本語 and English.",
        "try": "4. What to try (about 30 minutes)",
        "notes": "5. Please note",
        "role": "Role", "email": "Email address", "app": "App",
        "device": "Device", "req": "Requirements",
        "unsigned": "The warnings appear because the trial is not yet code-signed. The release version will be.",
        "server": "Trial server",
    },
}

INSTALL = [
    ("Windows", {"ja": "Windows 10 / 11（64ビット）", "en": "Windows 10 / 11, 64-bit"}, {
        "ja": ["ChuoSogo-Trial-Setup.exe をダウンロードして開きます。",
               "青い画面「WindowsによってPCが保護されました」が出たら「詳細情報」→「実行」。",
               "「次へ」→「完了」。Procure と Shop の両方が入り、デスクトップに試用ガイドができます。",
               "デスクトップの「Chuo-Sogo Procure」を開き、役割のボタンを押します。"],
        "en": ["Download and open ChuoSogo-Trial-Setup.exe.",
               "If a blue “Windows protected your PC” screen appears, press “More info”, then “Run anyway”.",
               "Press “Next”, then “Finish”. Procure and Shop are both installed, and the guide is put on the desktop.",
               "Open “Chuo-Sogo Procure” on the desktop and press a role button."]}),
    ("Mac", {"ja": "macOS 10.15 以降（Appleチップ / Intel 両対応）", "en": "macOS 10.15+, Apple chip or Intel"}, {
        "ja": ["ChuoSogo-Procure.dmg をダウンロードして開きます。",
               "アプリのアイコンを「Applications（アプリケーション）」フォルダへドラッグします。",
               "アプリケーションフォルダでアプリを右クリック（control+クリック）→「開く」→「開く」。",
               "「開けません」と出た場合：システム設定 →「プライバシーとセキュリティ」→「このまま開く」。",
               "Shop も試す場合は ChuoSogo-Shop.dmg で同じ手順です。"],
        "en": ["Download and open ChuoSogo-Procure.dmg.",
               "Drag the app icon onto the “Applications” folder.",
               "In Applications, right-click (control-click) the app, choose “Open”, then “Open” again.",
               "If macOS says it cannot be opened: System Settings → “Privacy & Security” → “Open Anyway”.",
               "To try Shop as well, do the same with ChuoSogo-Shop.dmg."]}),
    ("Android", {"ja": "Android 7.0 以降（64ビット）", "en": "Android 7.0+, 64-bit"}, {
        "ja": ["ChuoSogo-Procure.apk をダウンロードします（Wi-Fi推奨）。",
               "ダウンロード完了の通知、または「ファイル」アプリから開きます。",
               "「この提供元のアプリを許可」をオンにして戻り、「インストール」を押します。",
               "Play プロテクトの確認が出たら「詳細」→「インストールする」。",
               "Shop も試す場合は ChuoSogo-Shop.apk で同じ手順です。"],
        "en": ["Download ChuoSogo-Procure.apk (Wi-Fi recommended).",
               "Open it from the download notification or the Files app.",
               "Turn on “Allow from this source”, go back and press “Install”.",
               "If Play Protect asks, choose “More details”, then “Install anyway”.",
               "To try Shop as well, do the same with ChuoSogo-Shop.apk."]}),
    ("Linux", {"ja": "x86_64（Ubuntu 22.04 以降など）", "en": "x86_64 (e.g. Ubuntu 22.04+)"}, {
        "ja": ["ChuoSogo-Procure.AppImage をダウンロードします。",
               "ファイルを右クリック →「プロパティ」→「実行を許可」（または chmod +x）。",
               "ダブルクリックで開きます。"],
        "en": ["Download ChuoSogo-Procure.AppImage.",
               "Right-click the file → Properties → “Allow executing” (or chmod +x).",
               "Double-click to open it."]}),
    ("iPhone / iPad", {"ja": "—", "en": "—"}, {
        "ja": ["準備中です（Appleの配布登録が必要なため）。パソコンかAndroidでお試しください。"],
        "en": ["Not available yet (it needs Apple distribution approval). Please use a computer or an Android phone."]}),
]

ACCOUNTS = [
    ("ceo@demo-customer.example", "顧客企業の管理者（サンプル精機株式会社）", "Customer company administrator (Sample Seiki)", "Procure"),
    ("manager@demo-customer.example", "承認者（部長）", "Approver (manager)", "Procure"),
    ("requester@demo-customer.example", "依頼者（現場担当）", "Requester (shop floor)", "Procure"),
    ("finance@demo-customer.example", "経理", "Finance", "Procure"),
    ("operator@demo.chuo-sogo.example", "中央総合の調達担当", "Chuo-Sogo buyer", "Procure"),
    ("sales@demo-supplier.example", "仕入先（メーカー営業）", "Supplier (sales)", "Procure"),
    ("hanako@demo-consumer.example", "一般のお客様", "Consumer", "Shop"),
]

TRY = {
    "ja": [
        ("A. 顧客企業（サンプル精機）", [
            "「依頼者」を選択 →「調達依頼」。下書き「安全手袋・消耗品」を開いて提出します。",
            "「調達マネージャー」を選択 →「承認」。内容を確認して承認または差し戻し。承認できるのは人だけで、AIは承認も発注もしません。",
            "「経理担当」を選択 →「中央総合からの請求書」。インボイス制度対応の請求書をPDFで保存できます。",
            "「組織管理者」を選択 →「メンバー管理」「監査ログ」。誰がいつ何をしたかがすべて残ります。"]),
        ("B. 中央総合の調達担当", [
            "「調達オペレーター」を選択 →「業務キュー」。顧客から届いた依頼が並びます。",
            "「見積依頼 (RFQ)」「見積書」で仕入先の見積を比較します。試用版ではメールは実際には送信されません。"]),
        ("C. 仕入先（大阪ベアリング）", [
            "「サプライヤー管理者」を選択 →「製品掲載」。写真・型番・仕様付きの製品が並んでいます。",
            "新しい製品を作り、AIアシストにカタログのPDFやExcelを渡すと入力案が出ます。採用するかは人が決め、入力済みの項目は上書きされません。"]),
        ("D. 一般のお客様（Shopアプリ）", [
            "Shopアプリで「個人のお客様」を選択。税込価格の商品一覧からカートに入れ、注文します。",
            "「ご注文履歴」で請求書付きのデモ注文を開き、振込先と請求書PDFを確認します。"]),
    ],
    "en": [
        ("A. Customer company (Sample Seiki)", [
            "Choose <b>Requester</b> → “Procurement”. Open the draft “Safety gloves and consumables” and submit it.",
            "Choose <b>Procurement manager</b> → “Approvals”. Check the request and approve it or send it back. Only people approve; the AI never approves or orders.",
            "Choose <b>Finance</b> → “Invoices from Chuo-Sogo”. Save a qualified (インボイス) invoice as PDF.",
            "Choose <b>Organisation administrator</b> → “Members” and “Audit log”. Every action is recorded with who and when."]),
        ("B. Chuo-Sogo buyer", [
            "Choose <b>Procurement operator</b> → “Operations queue”. Requests from customers appear here.",
            "Compare supplier quotations under “RFQs” and “Quotations”. In the trial no email is actually sent."]),
        ("C. Supplier (Osaka Bearing)", [
            "Choose <b>Supplier administrator</b> → “Listings”. Products with photos, model numbers and specifications.",
            "Create a product and give the AI assistant a catalogue PDF or Excel file: it suggests the fields. A person decides what to keep; filled-in fields are never overwritten."]),
        ("D. Consumer (Shop app)", [
            "In the Shop app, choose <b>Shopper</b>. Add products (tax-inclusive prices) to the cart and order.",
            "Open the demo order with an invoice under “My orders” and check the bank details and invoice PDF."]),
    ],
}

NOTES = {
    "ja": ["データはすべて架空です。実際の発注・請求・メール送信は一切行われません。",
           "自由に操作してかまいません。毎晩3時（日本時間）に初期状態へ戻ります。",
           "本物の社内情報や個人情報は入力しないでください。",
           "ご意見や不具合は、画面のスクリーンショットと一緒に担当者へお知らせください。"],
    "en": ["All data is fictional. No real orders, invoices or emails are sent.",
           "Try anything you like. The demo is reset every night at 03:00 Japan time.",
           "Please do not enter real company or personal information.",
           "Send feedback or problems to your contact, with a screenshot of the screen."],
}

CSS = """
@page { size: A4; margin: 16mm 15mm; }
* { box-sizing: border-box; }
body { margin: 0; color: #161b26; background: #fff;
  font: 14px/1.7 "Noto Sans CJK JP", "Noto Sans JP", "Hiragino Sans", "Yu Gothic UI", "Meiryo", system-ui, sans-serif; }
main { max-width: 820px; margin: 0 auto; padding: 32px 20px 56px; }
header { display: flex; gap: 14px; align-items: center; border-bottom: 2px solid #1d3461; padding-bottom: 14px; }
.mark { width: 40px; height: 40px; flex: none; display: grid; place-items: center; background: #1d3461; color: #fff;
  font-weight: 700; font-size: 22px; border-radius: 4px; }
h1 { font-size: 21px; margin: 0; line-height: 1.35; }
.sub { color: #667085; font-size: 12.5px; }
.lead { background: #f4f6fa; border-left: 3px solid #1d3461; padding: 10px 14px; margin: 18px 0; }
h2 { font-size: 16px; margin: 28px 0 10px; padding-bottom: 4px; border-bottom: 1px solid #dfe3e8; break-after: avoid; }
h3 { font-size: 14px; margin: 16px 0 6px; break-after: avoid; }
table { width: 100%; border-collapse: collapse; font-size: 13px; margin: 6px 0; }
th, td { border: 1px solid #dfe3e8; padding: 6px 9px; text-align: left; vertical-align: top; }
th { background: #f4f6fa; font-weight: 600; }
tr, li { break-inside: avoid; }
code { font-family: "Noto Sans Mono", ui-monospace, monospace; font-size: 12.5px; word-break: break-all; }
ol, ul { padding-left: 1.4em; margin: 4px 0; }
li { margin: 3px 0; }
.os { break-inside: avoid; border: 1px solid #dfe3e8; border-radius: 4px; padding: 8px 14px 10px; margin: 10px 0; }
.os h3 { margin: 4px 0; display: flex; justify-content: space-between; gap: 12px; flex-wrap: wrap; }
.os h3 span { color: #667085; font-weight: 400; font-size: 12.5px; }
.small { color: #667085; font-size: 12px; }
a { color: #1d4f91; }
"""


def page(lang: str) -> str:
    t = T[lang]
    e = html.escape
    out = [f'<!doctype html><html lang="{lang}"><head><meta charset="utf-8">'
           f'<meta name="viewport" content="width=device-width, initial-scale=1">'
           f'<title>{e(t["title"])}</title><style>{CSS}</style></head><body><main>',
           f'<header><div class="mark">中</div><div><h1>{e(t["title"])}</h1><div class="sub">{e(t["sub"])}</div></div></header>',
           f'<p class="lead">{e(t["lead"])}</p>',
           f'<h2>{e(t["get"])}</h2><p>{e(t["get_body"])}</p><p><a href="{PAGE}"><code>{PAGE}</code></a></p>',
           f'<h2>{e(t["install"])}</h2>']
    for name, req, steps in INSTALL:
        items = "".join(f"<li>{e(s)}</li>" for s in steps[lang])
        out.append(f'<div class="os"><h3>{e(name)} <span>{e(req[lang])}</span></h3><ol>{items}</ol></div>')
    out.append(f'<p class="small">{e(t["unsigned"])}</p>')
    out.append(f'<h2>{e(t["signin"])}</h2><p>{e(t["signin_body"])}</p>')
    out.append(f'<table><tr><th>{e(t["role"])}</th><th>{e(t["email"])}</th><th>{e(t["app"])}</th></tr>')
    for email, ja, en, app in ACCOUNTS:
        out.append(f"<tr><td>{e(ja if lang == 'ja' else en)}</td><td><code>{e(email)}</code></td><td>{app}</td></tr>")
    out.append("</table>")
    out.append(f'<h2>{e(t["try"])}</h2>')
    for head, items in TRY[lang]:  # items carry <b> only, written here
        out.append(f"<h3>{e(head)}</h3><ol>" + "".join(f"<li>{i}</li>" for i in items) + "</ol>")
    out.append(f'<h2>{e(t["notes"])}</h2><ul>' + "".join(f"<li>{e(n)}</li>" for n in NOTES[lang]) + "</ul>")
    out.append(f'<p class="small">{e(t["server"])}: <code>{DEMO}</code></p></main></body></html>')
    return "\n".join(out)


def chrome() -> str:
    if os.environ.get("CHROME"):
        return os.environ["CHROME"]
    found = sorted(glob.glob(os.path.expanduser("~/.cache/ms-playwright/chromium-*/chrome-linux*/chrome")))
    if not found:
        sys.exit("no Chromium found: set CHROME=/path/to/chrome")
    return found[-1]


def main() -> None:
    out = sys.argv[1] if len(sys.argv) > 1 else "."
    os.makedirs(out, exist_ok=True)
    browser = chrome()
    for lang in ("ja", "en"):
        src = os.path.abspath(os.path.join(out, f"guide-{lang}.html"))
        with open(src, "w", encoding="utf-8") as f:
            f.write(page(lang))
        pdf = os.path.abspath(os.path.join(out, f"ChuoSogo-Trial-Guide-{lang}.pdf"))
        subprocess.run([browser, "--headless", "--no-sandbox", "--disable-gpu", "--no-pdf-header-footer",
                        f"--print-to-pdf={pdf}", "file://" + src], check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        print(pdf)


if __name__ == "__main__":
    main()
