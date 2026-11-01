/* global document -- used inside b.execute(), which runs in the app's web view */
// Phase C end-to-end across both apps, run with scripts/e2e-catalogue.sh:
//   supplier (business app): creates a listing, uploads two real photos, submits
//   Chuo-Sogo (admin console): sees it in the review queue with photos, approves
//   customer (business app): finds it in the catalogue by model number, sees the photo
import { createHmac } from "node:crypto";
import { remote } from "webdriverio";

const { E2E_BUSINESS_APP, E2E_ADMIN_APP, E2E_OUT, E2E_SEED, E2E_PHOTOS } = process.env;
const seed = JSON.parse(E2E_SEED);
const results = [];
function check(name, cond) {
  results.push({ name, pass: Boolean(cond) });
  console.log(`${cond ? "PASS" : "FAIL"} ${name}`);
}

function base32Decode(s) {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  let bits = 0;
  let value = 0;
  const out = [];
  for (const c of s.replace(/[\s=]/g, "").toUpperCase()) {
    value = ((value << 5) | alphabet.indexOf(c)) & 0xffff;
    bits += 5;
    if (bits >= 8) {
      out.push((value >>> (bits - 8)) & 0xff);
      bits -= 8;
    }
  }
  return Buffer.from(out);
}
function totp(secret) {
  const msg = Buffer.alloc(8);
  msg.writeBigUInt64BE(BigInt(Math.floor(Date.now() / 1000 / 30)));
  const h = createHmac("sha1", base32Decode(secret)).update(msg).digest();
  const o = h[19] & 15;
  return String((h.readUInt32BE(o) & 0x7fffffff) % 1_000_000).padStart(6, "0");
}

let b;
const open = async (app) =>
  (b = await remote({
    hostname: "127.0.0.1",
    port: 4444,
    logLevel: "error",
    capabilities: { "tauri:options": { application: app } },
  }));
const lit = (s) => (s.includes("'") ? `"${s}"` : `'${s}'`);
async function click(text) {
  const xpath = `//button[normalize-space(.)=${lit(text)}]`;
  await b.waitUntil(async () => (await b.$$(xpath)).length > 0, {
    timeout: 15000,
    timeoutMsg: `no button "${text}"`,
  });
  const el = (await b.$$(xpath))[0];
  await el.waitForClickable({ timeout: 15000 });
  await el.click();
}
async function field(label, tag = "input") {
  const el = await b.$(`//label[starts-with(normalize-space(.), ${lit(label)})]//${tag}`);
  await el.waitForExist({ timeout: 15000 });
  return el;
}
async function nav(text) {
  const el = await b.$(`//ul[contains(@class,'nav-list')]//button[normalize-space(.)=${lit(text)}]`);
  await el.waitForClickable({ timeout: 15000 });
  await el.click();
}
async function seeText(selector, text) {
  return b
    .waitUntil(
      async () =>
        b.execute(
          (sel, txt) => Array.from(document.querySelectorAll(sel)).some((e) => e.textContent.trim() === txt),
          selector,
          text,
        ),
      { timeout: 20000, timeoutMsg: `"${text}" never appeared in ${selector}` },
    )
    .then(() => true);
}
/** Waits for at least `n` real (decoded, non-empty) images inside `scope`. */
async function images(scope, n) {
  return b
    .waitUntil(
      async () =>
        b
          .execute(
            (sel) =>
              Array.from(document.querySelectorAll(`${sel} img`)).filter(
                (i) => i.src.startsWith("data:image/") && i.complete && i.naturalWidth > 0,
              ).length,
            scope,
          )
          .then((count) => count >= n),
      { timeout: 20000, timeoutMsg: `fewer than ${n} photos shown in ${scope}` },
    )
    .then(() => true);
}
async function signIn(email) {
  const e = await b.$('input[type="email"]');
  await e.waitForExist({ timeout: 15000 });
  await e.setValue(email);
  await (await b.$('input[type="password"]')).setValue(seed.password);
  await (await b.$('button[type="submit"]')).click();
}
async function english() {
  await (await b.$(".language-menu > button")).click();
  await (await b.$('button[role="option"][lang="en"]')).click();
}
const shot = (name) => b.saveScreenshot(`${E2E_OUT}/${name}.png`);
const MODEL = "6204-2RS";

try {
  // --- Supplier ----------------------------------------------------------------------
  await open(E2E_BUSINESS_APP);
  await signIn(seed.supplier);
  await (await b.$(".nav-list")).waitForExist({ timeout: 15000 });
  await english();
  check("supplier workspace shows product listings", await seeText(".brand-workspace", "Supplier workspace"));
  await nav("Product listings");
  await click("New listing");
  // AI assistant: a voice note (converted on the server) fills the main fields...
  await click("Voice note");
  const voice = await b.$('[data-testid="voice-input"]');
  await b.execute((el) => {
    el.hidden = false;
  }, voice);
  await voice.setValue(`${E2E_PHOTOS}/memo.ogg`);
  check(
    "voice note is transcribed on the server",
    await seeText(
      '[data-testid="ai-input-text"]',
      "メーカー: NSK\n型番: 6204-2RS\n品名: 深溝玉軸受 6204-2RS\n状態: 新品\n内径: 20 mm",
    ),
  );
  await click("Apply to the form");
  await b.waitUntil(async () => (await (await field("Model / part number")).getValue()) === MODEL, {
    timeout: 15000,
    timeoutMsg: "suggestion not applied",
  });
  check(
    "the suggestion fills the form (manufacturer, model, name)",
    (await (await field("Manufacturer")).getValue()) === "NSK" &&
      (await (await field("Product name (Japanese)")).getValue()) === "深溝玉軸受 6204-2RS",
  );
  // ...and a catalogue file (Shift_JIS CSV) adds specifications.
  await click("Catalogue file");
  const cat = await b.$('[data-testid="catalogue-file-input"]');
  await b.execute((el) => {
    el.hidden = false;
  }, cat);
  await cat.setValue(`${E2E_PHOTOS}/catalogue.csv`);
  await (await b.$('[data-testid="ai-suggestion"]')).waitForExist({ timeout: 20000 });
  await click("Apply to the form");
  await b.waitUntil(async () => (await b.$$('[aria-label="Property"]')).length >= 3, {
    timeout: 15000,
    timeoutMsg: "specs not merged",
  });
  check("catalogue file adds specifications (20 mm, 47 mm, 14 mm)", true);
  check(
    "a second suggestion never overwrites what is already filled",
    (await (await field("Product name (Japanese)")).getValue()) === "深溝玉軸受 6204-2RS",
  );
  await shot("00-ai-assistant");
  await (await field("Description (Japanese)", "textarea")).setValue("両側ゴムシール付き。即納在庫。");
  await (await field("Business price (JPY, excl. tax)")).setValue("480");
  await (await field("Stock")).setValue("120");
  await click("Save");
  check(
    "the checklist asks for photos first",
    await seeText('[data-testid="listing-missing"] li', "at least 2 photos"),
  );
  const input = await b.$('[data-testid="photo-input"]');
  await b.execute((el) => {
    el.hidden = false;
  }, input);
  await input.setValue(`${E2E_PHOTOS}/front.jpg`);
  await images('[data-testid="listing-media"]', 1);
  await input.setValue(`${E2E_PHOTOS}/side.jpg`);
  check("both photos are uploaded and shown", await images('[data-testid="listing-media"]', 2));
  await shot("01-supplier-listing");
  await click("Submit for review");
  check("listing is in review", await seeText(".page-header .badge", "In review"));
  await b.deleteSession();

  // --- Chuo-Sogo moderation (admin console) ------------------------------------------
  await open(E2E_ADMIN_APP);
  await signIn(seed.root);
  const secretEl = await b.$('[data-testid="totp-secret"]');
  await secretEl.waitForExist({ timeout: 15000 });
  const secret = (await secretEl.getText()).replace(/\s/g, "");
  await (await b.$('[data-testid="totp-code"]')).setValue(totp(secret));
  await (await b.$('button[type="submit"]')).click();
  await (await b.$(".nav-list")).waitForExist({ timeout: 15000 });
  await english();
  await nav("Listings");
  await seeText('[data-testid="moderation-list"] td', seed.supplierName);
  await click("深溝玉軸受 6204-2RS");
  check("moderator sees the supplier's photos", await images('[data-testid="listing-view"]', 2));
  await shot("02-moderation");
  await click("Approve and publish");
  check("listing is published", await seeText(".page-header .badge", "Published"));
  await b.deleteSession();

  // --- Customer catalogue ------------------------------------------------------------
  await open(E2E_BUSINESS_APP);
  await signIn(seed.customer);
  await (await b.$(".nav-list")).waitForExist({ timeout: 15000 });
  await english();
  await nav("Catalogue");
  await (await b.$('input[type="search"]')).setValue("6204 2rs");
  await click("Search");
  check(
    "customer finds the product by model number",
    await seeText(".product-card strong", "深溝玉軸受 6204-2RS"),
  );
  check("the catalogue shows the cover photo", await images('[data-testid="catalogue-grid"]', 1));
  await (await b.$(".product-card")).click();
  check("product page shows the specification", await seeText('[data-testid="listing-specs"] td', "20 mm"));
  await shot("03-catalogue");
} catch (e) {
  console.log(`FAIL unexpected error: ${e.message}`);
  const alerts = await b
    ?.execute(() => Array.from(document.querySelectorAll("[role=alert]")).map((e) => e.textContent))
    .catch(() => []);
  if (alerts?.length) console.log(`  on-screen errors: ${alerts.join(" | ")}`);
  results.push({ name: "no unexpected error", pass: false });
  await shot("99-failure").catch(() => {});
} finally {
  await b?.deleteSession().catch(() => {});
}

const failed = results.filter((r) => !r.pass).length;
console.log(`${results.length - failed}/${results.length} passed`);
process.exit(failed ? 1 : 0);
