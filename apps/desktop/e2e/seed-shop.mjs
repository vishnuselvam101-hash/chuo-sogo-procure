// Seeds a throwaway backend for e2e/shop.mjs through the public and admin APIs: issuer
// details, an open shop with its seller notice, and one approved listing with a consumer
// price (¥1,100 incl. tax, stock 5) and two photos. Prints what the test needs as JSON,
// including an admin-console session for the staff steps.
//   E2E_API, E2E_OUTBOX, E2E_ROOT_TOKEN: as for seed.mjs; E2E_PHOTOS: directory with front.jpg, side.jpg
import { createHmac } from "node:crypto";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const { E2E_API, E2E_OUTBOX, E2E_ROOT_TOKEN, E2E_PHOTOS } = process.env;
const PASSWORD = "e2e-correct-horse-battery";

async function call(method, path, token, body) {
  const res = await fetch(`${E2E_API}${path}`, {
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
  for (const f of readdirSync(E2E_OUTBOX).sort().reverse()) {
    const text = readFileSync(join(E2E_OUTBOX, f), "utf8");
    if (text.includes(`To: ${email}\n`)) return /^token: (\S+)$/m.exec(text)[1];
  }
  throw new Error(`no mail for ${email}`);
}
async function activate(email, token = mailedToken(email)) {
  await v1("POST", "/auth/set-password", null, { token, password: PASSWORD });
  return (await v1("POST", "/auth/login", null, { email, password: PASSWORD })).token;
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

const root = await activate("root@chuo-sogo.e2e", E2E_ROOT_TOKEN);
const step = await admin("POST", "/auth/login", null, { email: "root@chuo-sogo.e2e", password: PASSWORD });
const { secret } = await admin("POST", "/auth/totp/enroll", null, { token: step.token });
const staff = (await admin("POST", "/auth/totp/confirm", null, { token: step.token, code: totp(secret) }))
  .token;

await admin("PUT", "/billing/issuer", staff, {
  legal_name: "株式会社中央総合",
  registration_number: "T7000012050002",
  address: "大阪府大阪市北区梅田1-1-1",
  bank_details: "テスト銀行 本店 普通 1234567 カ）チュウオウソウゴウ",
  payment_terms_days: 30,
  tax_rounding: "floor",
  number_prefix: "CS",
});
await admin("PUT", "/shop/settings", staff, {
  open: true,
  shipping_fee_minor: 880,
  free_shipping_over_minor: 10000,
  payment_days: 7,
  seller_name: "株式会社中央総合",
  responsible_person: "代表取締役 山田太郎",
  seller_address: "大阪府大阪市北区梅田1-1-1",
  seller_phone: "06-0000-0000",
  seller_email: "shop@chuo-sogo.e2e",
  price_info: "表示価格はすべて税込です。",
  payment_info: "銀行振込（ご注文確定後7日以内）",
  delivery_info: "ご入金確認後3営業日以内に発送します。",
  returns_policy: "未使用品に限り、到着後8日以内にご連絡ください。",
  privacy_policy: "お客様の情報は配送とご連絡にのみ使用します。",
});

const { org_id: supplierOrg } = await v1("POST", "/platform/organisations", root, {
  name: "大阪ベアリング株式会社",
  kind: "supplier",
  admin_email: "sales@bearing.e2e",
  admin_name: "営業",
});
const supplier = await activate("sales@bearing.e2e");
await v1("POST", `/invitations/${supplierOrg}/accept`, supplier);
const base = `/orgs/${supplierOrg}/listings`;
const { id } = await v1("POST", base, supplier, {
  manufacturer: "NSK",
  model_number: "6204-2RS",
  condition: "new",
  title: "深溝玉軸受 6204-2RS",
  description: "両側ゴムシール付き。",
  specs: [{ key: "内径", value: "20 mm" }],
  business_price_minor: 800,
  consumer_price_minor: 1100,
  tax_rate_bps: 1000,
  stock: 5,
});
for (const f of ["front.jpg", "side.jpg"]) {
  await v1("POST", `${base}/${id}/media`, supplier, {
    kind: "photo",
    file_name: f,
    data_base64: readFileSync(join(E2E_PHOTOS, f)).toString("base64"),
  });
}
await v1("POST", `${base}/${id}/submit`, supplier, {});
await admin("POST", `/listings/${id}/approve`, staff, {});

console.log(JSON.stringify({ password: PASSWORD, staff, product: "深溝玉軸受 6204-2RS" }));
