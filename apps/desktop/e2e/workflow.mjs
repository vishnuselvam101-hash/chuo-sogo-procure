/* global document -- used inside b.execute(), which runs in the app's web view */
// Phase 2 end-to-end: one procurement request through the real desktop app and backend,
// by three different people. Run with scripts/e2e-workflow.sh.
//   requester: drafts and submits a request
//   operator (Chuo-Sogo, assigned): picks up, verifies the item with evidence, sources,
//     dispatches the RFQ in test mode, records two quotations, compares, recommends, sends
//   procurement manager (customer): approves
import { remote } from "webdriverio";

const { E2E_APP, E2E_OUT, E2E_SEED } = process.env;
const seed = JSON.parse(E2E_SEED);
const results = [];
function check(name, cond) {
  results.push({ name, pass: Boolean(cond) });
  console.log(`${cond ? "PASS" : "FAIL"} ${name}`);
}

const b = await remote({
  hostname: "127.0.0.1",
  port: 4444,
  logLevel: "error",
  capabilities: { "tauri:options": { application: E2E_APP } },
});

const buttonXpath = (text) => `//button[normalize-space(.)='${text}']`;
/** Clicks the first button with exactly this text (e.g. the top-ranked "Recommend"). */
async function click(text) {
  await b.waitUntil(async () => (await b.$$(buttonXpath(text))).length > 0, {
    timeout: 15000,
    timeoutMsg: `no button "${text}"`,
  });
  const el = (await b.$$(buttonXpath(text)))[0];
  await el.waitForClickable({ timeout: 15000 });
  await el.click();
}
/** Input/select/textarea inside the label whose own text starts with `label`. */
async function field(label, tag = "input", scope = "") {
  const el = await b.$(`${scope}//label[starts-with(normalize-space(.), '${label}')]//${tag}`);
  await el.waitForExist({ timeout: 15000 });
  return el;
}
async function headerStatus(expected) {
  const badge = await b.$(".page-header .badge");
  await badge.waitUntil(async () => (await badge.getText()) === expected, {
    timeout: 15000,
    timeoutMsg: `status never became ${expected}`,
  });
  return true;
}
async function signIn(email) {
  const e = await b.$('input[type="email"]');
  await e.waitForExist({ timeout: 15000 });
  await e.setValue(email);
  await (await b.$('input[type="password"]')).setValue(seed.password);
  await (await b.$('button[type="submit"]')).click();
  await (await b.$(".nav-list")).waitForExist({ timeout: 15000 });
}
async function signOut() {
  await click("Sign out");
}
async function nav(text) {
  const el = await b.$(`//ul[contains(@class,'nav-list')]//button[normalize-space(.)='${text}']`);
  await el.waitForClickable({ timeout: 15000 });
  await el.click();
}
const shot = (name) => b.saveScreenshot(`${E2E_OUT}/${name}.png`);

try {
  // --- Requester -------------------------------------------------------------------
  await signIn("requester@customer.e2e");
  await (await b.$(".language-menu > button")).click();
  await (await b.$('button[role="option"][lang="en"]')).click();
  await nav("Procurement");
  await click("New procurement request");
  await (await field("Title")).setValue("Replacement bearings for line 3");
  await (
    await field("Description", "input", "//div[contains(@class,'item-fields')]")
  ).setValue("Deep groove ball bearing");
  await (await field("Manufacturer")).setValue("NSK");
  await (await field("Part number")).setValue("6204ZZ");
  await (await field("Quantity")).setValue("100");
  await click("Save as draft");
  check("requester creates a draft request", await headerStatus("Draft"));
  const heading = await (await b.$(".page-header h1")).getText();
  const number = heading.split(" — ")[0];
  check("request gets a PR number", /^PR-\d{4}-\d{6}$/.test(number));
  await click("Submit to Chuo-Sogo");
  check("requester submits it", await headerStatus("Submitted"));
  await shot("01-requester-submitted");
  await signOut();

  // --- Operator ----------------------------------------------------------------------
  await signIn("operator@chuo-sogo.e2e");
  await (await b.$(".org-switch select")).selectByVisibleText(seed.orgName);
  check(
    "operator switches into the assigned customer org",
    (await (await b.$(".brand-workspace")).getText()) === "Serving customer",
  );
  await nav("Procurement");
  await click(number);
  await click("Pick up");
  check("operator picks the request up", await headerStatus("Identifying products"));
  await click("Verify");
  await (await field("Evidence")).setValue("https://www.nsk.example/6204ZZ-datasheet.pdf");
  await click("Confirm verified");
  await (await b.$("//td//span[normalize-space(.)='Verified']")).waitForExist({ timeout: 15000 });
  await click("Start sourcing");
  check("sourcing only after verification", await headerStatus("Sourcing"));

  for (const s of ["Tokyo Bearings KK", "Osaka Motion Ltd"]) {
    await (await b.$(`//label[contains(normalize-space(.), '${s}')]/input`)).click();
  }
  await click("Create RFQ");
  await click("Dispatch (test mode)");
  check("RFQ dispatched in test mode", await headerStatus("RFQ sent"));
  await click("Show messages");
  await b.waitUntil(async () => (await b.$$("details")).length === 2, { timeout: 15000 });
  const messages = await b.execute(() =>
    Array.from(document.querySelectorAll("details pre")).map((e) => e.textContent ?? ""),
  );
  const outbox = messages.join("\n");
  check("one test-mode message per supplier", messages.length === 2);
  check(
    "supplier message never names the customer",
    !outbox.includes(seed.orgName) && outbox.includes("RFQ-"),
  );
  await shot("02-operator-rfq");

  for (const [supplier, price, lead] of [
    ["Tokyo Bearings KK", "1200", "7"],
    ["Osaka Motion Ltd", "1300", "14"],
  ]) {
    await click("Record quotation");
    await (await field("Supplier", "select")).selectByVisibleText(supplier);
    await (await b.$("input[aria-label='Unit price']")).setValue(price);
    await (await b.$("input[aria-label='Lead time (days)']")).setValue(lead);
    await click("Save");
    await (await b.$("input[aria-label='Unit price']")).waitForExist({ timeout: 15000, reverse: true });
  }
  check("two quotations recorded", await headerStatus("Quotes received"));
  await click("Run comparison");
  check("comparison runs", await headerStatus("Under comparison"));
  const firstRow = await b.$("//h3[normalize-space(.)='Comparison']/following::table[1]//tbody/tr[1]");
  await firstRow.waitForExist({ timeout: 15000 });
  const top = await firstRow.getText();
  check(
    "cheapest compliant quotation ranks first (exact total ¥120,000)",
    top.includes("Tokyo Bearings") && top.includes("120,000"),
  );
  await shot("03-operator-comparison");
  await click("Recommend");
  check("operator recommends", await headerStatus("Recommended"));
  await click("Send to customer for approval");
  check("recommendation sent for approval", await headerStatus("Awaiting approval"));
  check("operator is never offered an Approve button", (await b.$$(buttonXpath("Approve"))).length === 0);
  await signOut();

  // --- Customer approver ---------------------------------------------------------------
  await signIn("manager@customer.e2e");
  await nav("Approvals");
  await click(number);
  const page = await (await b.$("main")).getText();
  check("customer never sees internal supplier notes", !page.includes("INTERNAL-NOTE-DO-NOT-SHOW"));
  await shot("04-manager-approval");
  await click("Approve");
  check("customer approves; request approved", await headerStatus("Approved"));
  await shot("05-approved");
  await signOut();
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
