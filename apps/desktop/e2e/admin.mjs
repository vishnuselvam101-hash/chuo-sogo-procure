/* global document, HTMLInputElement -- used inside b.execute(), which runs in the app's web view */
// Phase A end-to-end: the admin console release build against a real backend.
// Run with scripts/e2e-admin.sh.
//   first sign-in enrols an authenticator (TOTP computed here from the key the app shows),
//   then: health overview, organisation rules, staff assignment, audit log, suspend and
//   reactivate, account deactivation, sign-out and a second sign-in with a code.
import { createHmac } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { remote } from "webdriverio";

const { E2E_APP, E2E_OUT, E2E_SEED, E2E_HOME } = process.env;
const seed = JSON.parse(E2E_SEED);
const results = [];
function check(name, cond) {
  results.push({ name, pass: Boolean(cond) });
  console.log(`${cond ? "PASS" : "FAIL"} ${name}`);
}

// RFC 6238 TOTP (HMAC-SHA1, 6 digits, 30 s), as an authenticator app computes it.
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
const step = (ms = Date.now()) => Math.floor(ms / 1000 / 30);
function totp(secret, s = step()) {
  const msg = Buffer.alloc(8);
  msg.writeBigUInt64BE(BigInt(s));
  const h = createHmac("sha1", base32Decode(secret)).update(msg).digest();
  const o = h[19] & 15;
  return String((h.readUInt32BE(o) & 0x7fffffff) % 1_000_000).padStart(6, "0");
}
/** A code from a time step the server has not seen yet (codes cannot be replayed). */
async function freshCode(secret, after) {
  while (step() <= after) await new Promise((r) => setTimeout(r, 1000));
  return { code: totp(secret), at: step() };
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
async function nav(text) {
  const el = await b.$(`//ul[contains(@class,'nav-list')]//button[normalize-space(.)=${lit(text)}]`);
  await el.waitForClickable({ timeout: 15000 });
  await el.click();
}
async function tab(text) {
  const el = await b.$(`//div[@role='tablist']//button[normalize-space(.)=${lit(text)}]`);
  await el.waitForClickable({ timeout: 15000 });
  await el.click();
}
/** Waits until some element matching `selector` has exactly `text`. */
async function seeText(selector, text) {
  return b
    .waitUntil(
      async () =>
        b.execute(
          (sel, txt) => Array.from(document.querySelectorAll(sel)).some((e) => e.textContent.trim() === txt),
          selector,
          text,
        ),
      { timeout: 15000, timeoutMsg: `"${text}" never appeared in ${selector}` },
    )
    .then(() => true);
}
async function password() {
  const e = await b.$('input[type="email"]');
  await e.waitForExist({ timeout: 15000 });
  await e.setValue(seed.root);
  await (await b.$('input[type="password"]')).setValue(seed.password);
  await (await b.$('button[type="submit"]')).click();
}
async function enterCode(code) {
  const input = await b.$('[data-testid="totp-code"]');
  await input.waitForExist({ timeout: 15000 });
  await input.setValue(code);
  await (await b.$('button[type="submit"]')).click();
}
/** Sets an input by its aria-label in the n-th table row (0-based), React-safe. */
async function cell(row, label, value) {
  const els = await b.$$(`[data-testid="invoice-editor"] tbody tr`);
  const el = await els[row].$(`[aria-label="${label}"]`);
  if ((await el.getTagName()) === "select") await el.selectByAttribute("value", value);
  else await el.setValue(value);
}
/** Date inputs: set through the native setter so React sees the change. */
async function setDate(label, value) {
  const el = await field(label);
  await b.execute(
    (input, v) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
      setter.call(input, v);
      input.dispatchEvent(new Event("input", { bubbles: true }));
    },
    el,
    value,
  );
}
const shot = (name) => b.saveScreenshot(`${E2E_OUT}/${name}.png`);

try {
  // --- First sign-in: enrol an authenticator ----------------------------------------
  await password();
  const secretEl = await b.$('[data-testid="totp-secret"]');
  await secretEl.waitForExist({ timeout: 15000 });
  const secret = (await secretEl.getText()).replace(/\s/g, "");
  check("first sign-in shows a new authenticator key", /^[A-Z2-7]{32}$/.test(secret));
  await shot("01-enrol");
  let used = step();
  await enterCode(totp(secret, used));
  await (await b.$('[data-testid="health"]')).waitForExist({ timeout: 15000 });
  check("confirming the code opens the console", true);
  await (await b.$(".language-menu > button")).click();
  await (await b.$('button[role="option"][lang="en"]')).click();
  check("overview shows system health", await seeText(".stats dt", "Server version"));
  await shot("02-overview");

  // --- Organisation rules ------------------------------------------------------------
  await nav("Organisations");
  await click(seed.org);
  await tab("Rules");
  const sla = await field("Product identification target (hours)");
  await sla.setValue("48");
  const tax = await field("Default consumption tax rate (%)");
  await tax.setValue("8");
  await click("Save");
  check("rules are saved", await seeText('[data-testid="org-rules"] [role="status"]', "Saved."));
  await tab("Members");
  await tab("Rules");
  await b.waitUntil(
    async () => (await (await field("Product identification target (hours)")).getValue()) === "48",
    {
      timeout: 15000,
      timeoutMsg: "saved rule not shown after reload",
    },
  );
  check(
    "saved rules load again from the server",
    (await (await field("Default consumption tax rate (%)")).getValue()) === "8",
  );
  await shot("03-rules");

  // --- Staff assignment ----------------------------------------------------------------
  await tab("Staff");
  const select = await b.$('select[aria-label="Choose a staff member"]');
  await select.waitForExist({ timeout: 15000 });
  await b.waitUntil(async () => (await select.$$("option")).length > 1, { timeout: 15000 });
  await select.selectByIndex(1);
  await click("Assign");
  check(
    "operator is assigned to the organisation",
    await seeText('[data-testid="org-staff"] td', seed.operator),
  );

  // --- Audit log ------------------------------------------------------------------------
  await tab("Audit log");
  await (await b.$('[data-testid="audit-chain"]')).waitForExist({ timeout: 15000 });
  check(
    "audit chain is intact",
    (await (await b.$('[data-testid="audit-chain"]')).getAttribute("class")) === "ok",
  );
  check("rule change is in the organisation's audit log", await seeText("td code", "org.settings_updated"));
  await shot("04-audit");

  // --- Suspend and reactivate ---------------------------------------------------------
  await tab("Profile");
  await click("Suspend organisation");
  await click(`Confirm: suspend ${seed.org}`);
  check("organisation is suspended", await seeText("p.muted .badge", "Suspended"));
  await click("Reactivate organisation");
  check("organisation is active again", await seeText("p.muted .badge", "Active"));

  // --- Accounts -------------------------------------------------------------------------
  await nav("Accounts");
  const search = await b.$('input[type="search"]');
  await search.waitForExist({ timeout: 15000 });
  await search.setValue("operator");
  await click("Search");
  await seeText('[data-testid="account-list"] td', seed.operator);
  await click("Deactivate");
  await click(`Confirm: deactivate ${seed.operator}`);
  check(
    "account is deactivated",
    await seeText('[role="status"]', "Account deactivated; all its sessions ended."),
  );
  await click("Reactivate");
  check("account is reactivated", await seeText('[role="status"]', "Account reactivated."));
  await search.setValue("root");
  await click("Search");
  check(
    "own account cannot be changed",
    await seeText('[data-testid="account-list"] span', "Your own account"),
  );
  await shot("05-accounts");

  // --- Billing: issuer, invoice, issue, PDF, payment, credit note -------------------
  await nav("Billing");
  await click("Issuer details");
  await (await field("Legal name")).setValue("株式会社中央総合");
  await (await field("Qualified invoice registration number")).setValue("T7000012050002");
  await (await field("Address")).setValue("大阪府大阪市北区梅田1-1-1");
  await (await field("Bank transfer details", "textarea")).setValue("テスト銀行 本店 普通 1234567");
  await click("Save");
  await click("New invoice");
  await (await field("Customer", "select")).selectByVisibleText(seed.org);
  await cell(0, "Description", "調達代行サービス料");
  await cell(0, "Qty", "3");
  await cell(0, "Unit price", "333");
  await click("Add line");
  await cell(1, "Description", "飲料");
  await cell(1, "Qty", "2");
  await cell(1, "Unit price", "105");
  await cell(1, "Tax rate", "800");
  await click("Save draft");
  await (await b.$('[data-testid="invoice-total"]')).waitForExist({ timeout: 15000 });
  // 999 + 99 (10 %) and 210 + 16 (8 %, rounded once per rate) = 1,324.
  check(
    "server computes per-rate tax for the draft",
    (await (await b.$('[data-testid="invoice-total"]')).getText()).includes("1,324"),
  );
  await click("Issue");
  await click("Confirm: issue (cannot be changed afterwards)");
  const year = new Date(Date.now() + 9 * 3600 * 1000).getUTCFullYear();
  check("issuing assigns the first invoice number", await seeText(".page-header h1", `CS-${year}-00001`));
  await click("Download PDF");
  await seeText('[role="status"]', `Saved to ${E2E_HOME}/Downloads/CS-${year}-00001.pdf`);
  const pdfPath = `${E2E_HOME}/Downloads/CS-${year}-00001.pdf`;
  check(
    "the PDF is saved to Downloads",
    existsSync(pdfPath) && readFileSync(pdfPath).subarray(0, 5).toString() === "%PDF-",
  );
  await shot("07-invoice-issued");
  const today = new Date(Date.now() + 9 * 3600 * 1000).toISOString().slice(0, 10);
  await setDate("Paid on", today);
  await click("Mark as paid");
  check("payment is recorded", await seeText('[data-testid="invoice-view"] .badge', "Paid"));
  await click("New credit note");
  await (await field("Reason for the credit", "textarea")).setValue("返品（1式）");
  await cell(0, "Description", "サービス料 返還");
  await cell(0, "Qty", "1");
  await cell(0, "Unit price", "333");
  await click("Save draft");
  await click("Issue");
  await click("Confirm: issue (cannot be changed afterwards)");
  check("credit note gets its own number series", await seeText(".page-header h1", `CS-CN-${year}-00001`));
  await shot("08-credit-note");

  // --- Sign out, then a normal sign-in with a code ------------------------------------
  await click("Sign out");
  await password();
  // A code the server would not accept now (outside its ±1 step window).
  const valid = [-1, 0, 1].map((d) => totp(secret, step() + d));
  await enterCode(["000000", "111111", "222222"].find((c) => !valid.includes(c)));
  check(
    "a wrong code is refused with a clear message",
    await seeText('[role="alert"]', "Enter the 6-digit code from your authenticator app."),
  );
  const next = await freshCode(secret, used);
  used = next.at;
  await (await b.$('[data-testid="totp-code"]')).clearValue();
  await enterCode(next.code);
  await (await b.$('[data-testid="health"]')).waitForExist({ timeout: 15000 });
  check("second sign-in with a fresh code works", true);
  await shot("06-second-sign-in");
  await click("Sign out");
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
