/* global document -- used inside b.execute(), which runs in the app's web view */
// Phase E end-to-end: a consumer uses the Chuo-Sogo Shop app (release build) from sign-up to
// a shipped order. Chuo-Sogo's staff steps (confirm, payment, shipping) are done through the
// admin API with a real admin session, as the admin console would. Run with scripts/e2e-shop.sh.
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { remote } from "webdriverio";

const { E2E_APP, E2E_OUT, E2E_SEED, E2E_HOME, E2E_API, E2E_OUTBOX } = process.env;
const seed = JSON.parse(E2E_SEED);
const EMAIL = "hanako@example.e2e";
const results = [];
function check(name, cond) {
  results.push({ name, pass: Boolean(cond) });
  console.log(`${cond ? "PASS" : "FAIL"} ${name}`);
}

async function staff(method, path, body) {
  const res = await fetch(`${E2E_API}/api/admin/v1${path}`, {
    method,
    headers: { authorization: `Bearer ${seed.staff}`, "content-type": "application/json" },
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  if (!res.ok) throw new Error(`${method} ${path} -> ${res.status} ${text}`);
  return text ? JSON.parse(text) : null;
}
function mailedToken(email) {
  for (const f of readdirSync(E2E_OUTBOX).sort().reverse()) {
    const text = readFileSync(join(E2E_OUTBOX, f), "utf8");
    if (text.includes(`To: ${email}\n`) && text.includes("Template: activation"))
      return /^token: (\S+)$/m.exec(text)[1];
  }
  throw new Error(`no activation mail for ${email}`);
}

const b = await remote({
  hostname: "127.0.0.1",
  port: 4444,
  logLevel: "error",
  capabilities: { "tauri:options": { application: E2E_APP } },
});
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
async function seeText(selector, text) {
  return b
    .waitUntil(
      async () =>
        b.execute(
          (sel, txt) => Array.from(document.querySelectorAll(sel)).some((e) => e.textContent.includes(txt)),
          selector,
          text,
        ),
      { timeout: 20000, timeoutMsg: `"${text}" never appeared in ${selector}` },
    )
    .then(() => true);
}
const shot = (name) => b.saveScreenshot(`${E2E_OUT}/${name}.png`);

try {
  // --- Legal notice is public; sign up; activate; sign in -----------------------------
  await click("Legal notice");
  check(
    "seller notice is shown before signing in",
    await seeText('[data-testid="legal-notice"]', "株式会社中央総合"),
  );
  await click("Back");
  await click("Create an account");
  await (await field("Your name")).setValue("佐藤花子");
  await (await field("Email address")).setValue(EMAIL);
  await click("Create account");
  await (await b.$('[data-testid="shop-code"]')).waitForExist({ timeout: 15000 });
  await (await field("Code from the email")).setValue(mailedToken(EMAIL));
  await (await field("New password")).setValue(seed.password);
  await click("Set password");
  await (await b.$('[data-testid="shop-signin"]')).waitForExist({ timeout: 15000 });
  await (await field("Email address")).setValue(EMAIL);
  await (await field("Password")).setValue(seed.password);
  await (await b.$('button[type="submit"]')).click();
  check("new consumer signs in to the shop", await seeText(".shop-brand", "Chuo-Sogo Shop"));

  // --- Browse, add to cart, check out ------------------------------------------------
  await seeText('[data-testid="shop-products"]', seed.product);
  check(
    "product list shows the tax-inclusive consumer price",
    await seeText('[data-testid="shop-products"] .price', "1,100"),
  );
  await (await b.$(".product-card")).click();
  const qty = await field("Quantity");
  await qty.setValue("2");
  await click("Add to cart");
  await seeText('[role="status"]', "Added to your cart.");
  await click("Cart2");
  check("cart total includes shipping (¥2,200 + ¥880)", await seeText('[data-testid="cart-total"]', "3,080"));
  await (await field("Name")).setValue("佐藤花子");
  await (await field("Postal code")).setValue("100-0001");
  await (await field("Prefecture")).setValue("東京都");
  await (await field("Address")).setValue("千代田区千代田1-1");
  await (await field("Phone")).setValue("03-1234-5678");
  await shot("01-checkout");
  await click("Place order");
  check("order is placed", await seeText(".page-header .badge", "Placed"));
  const number = await (await b.$(".page-header h1")).getText();

  // --- Chuo-Sogo confirms: invoice with bank details ----------------------------------
  const orders = await staff("GET", "/shop/orders?status=placed");
  const order = orders.find((o) => o.number === number);
  check("staff see the order in their queue", Boolean(order));
  const confirmed = await staff("POST", `/shop/orders/${order.id}/action`, { action: "confirm" });
  check("confirming issues the invoice", confirmed.invoice_number?.startsWith("CS-"));
  await click("Back");
  await click(number);
  check(
    "consumer sees the transfer instructions with the invoice number",
    await seeText('[data-testid="pay-instructions"]', confirmed.invoice_number),
  );
  await click("Download PDF");
  const pdf = `${E2E_HOME}/Downloads/${confirmed.invoice_number}.pdf`;
  await seeText('[role="status"]', `${confirmed.invoice_number}.pdf`);
  check(
    "the invoice PDF is saved",
    existsSync(pdf) && readFileSync(pdf).subarray(0, 5).toString() === "%PDF-",
  );
  await shot("02-awaiting-payment");

  // --- Payment and shipping -----------------------------------------------------------
  const today = new Date(Date.now() + 9 * 3600 * 1000).toISOString().slice(0, 10);
  await staff("POST", `/shop/orders/${order.id}/action`, { action: "paid", paid_on: today });
  await staff("POST", `/shop/orders/${order.id}/action`, {
    action: "ship",
    carrier: "ヤマト運輸",
    tracking_number: "1234-5678-9012",
  });
  await click("Back");
  await click(number);
  check("consumer sees the order shipped", await seeText(".page-header .badge", "Shipped"));
  check("with the tracking number", await seeText("p code", "1234-5678-9012"));
  await shot("03-shipped");
  check(
    "the cart is empty after ordering",
    await (async () => {
      await click("Cart");
      return seeText("p.muted", "Your cart is empty.");
    })(),
  );
} catch (e) {
  console.log(`FAIL unexpected error: ${e.message}`);
  const alerts = await b
    .execute(() => Array.from(document.querySelectorAll("[role=alert]")).map((e) => e.textContent))
    .catch(() => []);
  if (alerts.length) console.log(`  on-screen errors: ${alerts.join(" | ")}`);
  results.push({ name: "no unexpected error", pass: false });
  await shot("99-failure").catch(() => {});
} finally {
  await b.deleteSession();
}

const failed = results.filter((r) => !r.pass).length;
console.log(`${results.length - failed}/${results.length} passed`);
process.exit(failed ? 1 : 0);
