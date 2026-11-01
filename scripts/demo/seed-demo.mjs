// Fills the DEMO server with sample data through its public and admin APIs (exactly as the
// apps would): a customer company with four roles and procurement requests, Chuo-Sogo staff,
// a supplier with approved products and photos, an open shop with a consumer order and
// invoices. Everything is fictional and marked as demo. Prints the demo accounts as JSON.
//   DEMO_API (http://127.0.0.1:8090), DEMO_OUTBOX, DEMO_ROOT_TOKEN, DEMO_PHOTOS, DEMO_PASSWORD
import { createHmac, randomBytes } from "node:crypto";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const { DEMO_API, DEMO_OUTBOX, DEMO_ROOT_TOKEN, DEMO_PHOTOS, DEMO_PASSWORD } = process.env;
const PASSWORD = DEMO_PASSWORD;
// The platform super-admin never shares the trial password: its own random one, kept only in
// the owner-only accounts file (and app sign-in for it only works from the admin networks).
const ROOT_PASSWORD = `Root-${randomBytes(18).toString("base64url")}`;

async function call(method, path, token, body) {
  const res = await fetch(`${DEMO_API}${path}`, {
    method,
    headers: {
      ...(token ? { authorization: `Bearer ${token}` } : {}),
      ...(body ? { "content-type": "application/json" } : {}),
    },
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  if (!res.ok) throw new Error(`${method} ${path} -> ${res.status} ${text}`);
  return text ? JSON.parse(text) : null;
}
const v1 = (m, p, t, b) => call(m, `/api/v1${p}`, t, b);
const admin = (m, p, t, b) => call(m, `/api/admin/v1${p}`, t, b);

function mailedToken(email) {
  for (const f of readdirSync(DEMO_OUTBOX).sort().reverse()) {
    const text = readFileSync(join(DEMO_OUTBOX, f), "utf8");
    if (text.includes(`To: ${email}\n`) && text.includes("Template: activation"))
      return /^token: (\S+)$/m.exec(text)[1];
  }
  throw new Error(`no activation mail for ${email}`);
}
async function activate(email, token = mailedToken(email), password = PASSWORD) {
  await v1("POST", "/auth/set-password", null, { token, password });
  return (await v1("POST", "/auth/login", null, { email, password })).token;
}
async function member(org, by, email, name, role) {
  await v1("POST", `/orgs/${org}/members`, by, { email, display_name: name, role });
  const s = await activate(email);
  await v1("POST", `/invitations/${org}/accept`, s);
  return s;
}
function totp(secret) {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  let bits = 0;
  let value = 0;
  const key = [];
  for (const c of secret) {
    value = ((value << 5) | alphabet.indexOf(c)) & 0xffff;
    bits += 5;
    if (bits >= 8) {
      key.push((value >>> (bits - 8)) & 0xff);
      bits -= 8;
    }
  }
  const msg = Buffer.alloc(8);
  msg.writeBigUInt64BE(BigInt(Math.floor(Date.now() / 30000)));
  const h = createHmac("sha1", Buffer.from(key)).update(msg).digest();
  const o = h[19] & 15;
  return String((h.readUInt32BE(o) & 0x7fffffff) % 1_000_000).padStart(6, "0");
}
const photo = (f) => readFileSync(join(DEMO_PHOTOS, f)).toString("base64");

// --- Chuo-Sogo (platform) ---------------------------------------------------------------
const ROOT = "admin@demo.chuo-sogo.example";
const root = await activate(ROOT, DEMO_ROOT_TOKEN, ROOT_PASSWORD);
const platform = (await v1("GET", "/me", root)).memberships[0].org_id;
const step = await admin("POST", "/auth/login", null, { email: ROOT, password: ROOT_PASSWORD });
const { secret, otpauth_uri } = await admin("POST", "/auth/totp/enroll", null, { token: step.token });
const staff = (await admin("POST", "/auth/totp/confirm", null, { token: step.token, code: totp(secret) })).token;
const operator = await member(platform, root, "operator@demo.chuo-sogo.example", "調達担当 鈴木", "procurement_operator");

await admin("PUT", "/billing/issuer", staff, {
  legal_name: "株式会社中央総合（デモ）",
  registration_number: "T7000012050002",
  address: "大阪府大阪市北区梅田1-1-1（デモ用の住所）",
  phone: "06-0000-0000",
  email: "billing@demo.chuo-sogo.example",
  bank_details: "デモ銀行 本店 普通 0000000 カ）チュウオウソウゴウ（デモ・振込不可）",
  payment_terms_days: 30,
  tax_rounding: "floor",
  number_prefix: "DEMO",
});
await admin("PUT", "/shop/settings", staff, {
  open: true,
  shipping_fee_minor: 880,
  free_shipping_over_minor: 10000,
  payment_days: 7,
  seller_name: "株式会社中央総合（デモ表示）",
  responsible_person: "デモ 責任者",
  seller_address: "大阪府大阪市北区梅田1-1-1（デモ）",
  seller_phone: "06-0000-0000",
  seller_email: "shop@demo.chuo-sogo.example",
  price_info: "表示価格はすべて税込です。これはデモ環境です。実際の販売は行いません。",
  payment_info: "銀行振込（デモ：実際のお振込みはしないでください）",
  delivery_info: "デモ環境のため、実際の発送はありません。",
  returns_policy: "デモ環境のため、返品の受付はありません。",
  privacy_policy: "デモ環境です。入力された情報はデモの確認にのみ使われ、定期的に削除されます。",
});

// --- Customer company (business app) ----------------------------------------------------
const { org_id: company } = await v1("POST", "/platform/organisations", root, {
  name: "サンプル精機株式会社",
  kind: "customer",
  admin_email: "ceo@demo-customer.example",
  admin_name: "代表取締役 サンプル",
  default_locale: "ja",
});
const ceo = await activate("ceo@demo-customer.example");
await v1("POST", `/invitations/${company}/accept`, ceo);
const manager = await member(company, ceo, "manager@demo-customer.example", "購買部長 山田", "procurement_manager");
const requester = await member(company, ceo, "requester@demo-customer.example", "製造課 田中", "requester");
await member(company, ceo, "finance@demo-customer.example", "経理 佐藤", "finance");
const operatorId = (await v1("GET", "/me", operator)).user.id;
await v1("POST", `/orgs/${company}/assignments`, root, { user_id: operatorId });

for (const [name, email] of [
  ["東京ベアリング株式会社（デモ）", "rfq@tokyo-bearings.example"],
  ["大阪モーション株式会社（デモ）", "sales@osaka-motion.example"],
]) {
  const { id } = await v1("POST", "/suppliers", operator, { name, country: "JP", email, categories: ["bearings"] });
  await v1("POST", `/suppliers/${id}/status`, operator, { status: "verified" });
  await v1("POST", `/orgs/${company}/suppliers/${id}/proposal`, operator);
  await v1("PUT", `/orgs/${company}/suppliers/${id}/decision`, manager, { decision: "approved" });
}

const needed = new Date(Date.now() + 21 * 86400000).toISOString().slice(0, 10);
const r1 = await v1("POST", `/orgs/${company}/requests`, requester, {
  title: "ライン3 ベアリング交換部品",
  description: "定期メンテナンス用。同等品可、納期優先。",
  currency: "JPY",
  needed_by: needed,
  delivery_location: "本社工場 資材倉庫",
  items: [
    { description: "深溝玉軸受", manufacturer: "NSK", mpn: "6204ZZ", quantity: 100, unit: "pcs" },
    { description: "リニアガイド", manufacturer: "THK", mpn: "HSR15", quantity: 4, unit: "set" },
  ],
});
await v1("POST", `/orgs/${company}/requests/${r1.id}/submit`, requester, {});
await v1("POST", `/orgs/${company}/requests`, requester, {
  title: "安全手袋・消耗品（下書き）",
  currency: "JPY",
  items: [{ description: "耐切創手袋 Lサイズ", quantity: 50, unit: "pcs" }],
});

// A service-fee invoice from Chuo-Sogo to the company.
const fee = await admin("POST", `/orgs/${company}/billing/invoices`, staff, {
  recipient_address: "愛知県名古屋市（デモ）",
  notes: "2026年10月分 調達代行サービス料（デモ）",
  lines: [
    { description: "調達代行サービス料 2026年10月分", quantity: 1, unit: "式", unit_price_minor: 50000, tax_rate_bps: 1000 },
    { description: "見積比較レポート作成", quantity: 3, unit: "件", unit_price_minor: 5000, tax_rate_bps: 1000 },
  ],
});
await admin("POST", `/orgs/${company}/billing/invoices/${fee.id}/issue`, staff, {});

// --- Supplier with products in the catalogue and the shop -------------------------------
const { org_id: supplier } = await v1("POST", "/platform/organisations", root, {
  name: "大阪ベアリング株式会社（デモ）",
  kind: "supplier",
  admin_email: "sales@demo-supplier.example",
  admin_name: "営業部 中村",
});
const sales = await activate("sales@demo-supplier.example");
await v1("POST", `/invitations/${supplier}/accept`, sales);
const products = [
  ["NSK", "6204-2RS", "深溝玉軸受 6204-2RS", "Deep groove ball bearing 6204-2RS", "両側ゴムシール付き。モーター・ポンプ等に。", [["内径", "20 mm"], ["外径", "47 mm"], ["幅", "14 mm"]], 480, 1100, 120, "bearing"],
  ["MITSUBISHI", "HG-KR43", "ACサーボモータ HG-KR43", "AC servo motor HG-KR43", "定格出力400W。中古・動作確認済み。", [["定格出力", "400 W"], ["定格回転数", "3000 r/min"]], 38000, null, 1, "servo"],
  ["THK", "HSR15A1SS", "LMガイド HSR15", "Linear guide HSR15", "レール長さ別途ご相談。", [["レール幅", "15 mm"]], 6200, 7480, 20, "guide"],
  ["ミドリ安全", "MG-500", "耐切創手袋 MG-500", "Cut-resistant gloves MG-500", "作業現場向け。洗濯可能。", [["サイズ", "L"], ["規格", "EN388 レベルC"]], 900, 1320, 300, "gloves"],
];
for (const [maker, model, title, titleEn, desc, specs, biz, consumer, stock, img] of products) {
  const used = model === "HG-KR43";
  const { id } = await v1("POST", `/orgs/${supplier}/listings`, sales, {
    manufacturer: maker,
    model_number: model,
    serial_number: used ? "SN-DEMO-0001" : null,
    condition: used ? "used" : "new",
    title,
    title_en: titleEn,
    description: desc,
    specs: specs.map(([key, value]) => ({ key, value })),
    business_price_minor: biz,
    consumer_price_minor: consumer,
    tax_rate_bps: 1000,
    stock,
  });
  for (const n of [1, 2]) {
    await v1("POST", `/orgs/${supplier}/listings/${id}/media`, sales, {
      kind: "photo",
      file_name: `${img}-${n}.jpg`,
      data_base64: photo(`${img}-${n}.jpg`),
    });
  }
  await v1("POST", `/orgs/${supplier}/listings/${id}/submit`, sales, {});
  await admin("POST", `/listings/${id}/approve`, staff, {});
}

// --- A consumer with an invoiced order ----------------------------------------------------
const CONSUMER = "hanako@demo-consumer.example";
await v1("POST", "/shop/signup", null, { email: CONSUMER, display_name: "佐藤 花子", locale: "ja" });
const hanako = await activate(CONSUMER);
const hanakoOrg = (await v1("GET", "/me", hanako)).memberships[0].org_id;
const forSale = await v1("GET", `/orgs/${hanakoOrg}/shop/products`, hanako);
const bearing = forSale.find((p) => p.model_number === "6204-2RS");
const order = await v1("POST", `/orgs/${hanakoOrg}/shop/orders`, hanako, {
  lines: [{ listing_id: bearing.id, quantity: 2 }],
  ship_to: { name: "佐藤 花子", postal_code: "100-0001", prefecture: "東京都", address_line: "千代田区千代田1-1（デモ）", phone: "03-0000-0000" },
  expected_total_minor: 2 * 1100 + 880,
});
await admin("POST", `/shop/orders/${order.id}/action`, staff, { action: "confirm" });

console.log(
  JSON.stringify({
    password: PASSWORD,
    admin: { email: ROOT, password: ROOT_PASSWORD, authenticator: otpauth_uri, secret },
    business: {
      company: "サンプル精機株式会社",
      ceo: "ceo@demo-customer.example",
      manager: "manager@demo-customer.example",
      requester: "requester@demo-customer.example",
      finance: "finance@demo-customer.example",
      operator: "operator@demo.chuo-sogo.example",
      supplier: "sales@demo-supplier.example",
    },
    shop: { consumer: CONSUMER },
  }),
);
