/* global document -- used inside b.execute(), which runs in the app's web view */
// Phase 2 end-to-end: one procurement request through the real desktop app and backend,
// by three different people. Run with scripts/e2e-workflow.sh.
//   requester: drafts and submits a request
//   operator (Chuo-Sogo, assigned): picks up, verifies the item with evidence, sources,
//     dispatches the RFQ in test mode, records two quotations, compares, recommends, sends
//   procurement manager (customer): approves
import { existsSync, readFileSync, writeFileSync } from "node:fs";
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
  // XPath string literal: double quotes when the label contains an apostrophe.
  const lit = label.includes("'") ? `"${label}"` : `'${label}'`;
  const el = await b.$(`${scope}//label[starts-with(normalize-space(.), ${lit})]//${tag}`);
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
  // Attach a drawing (a real local file through the file picker input).
  const drawing = `${E2E_OUT}/drawing-line3.pdf`;
  const pdf = "%PDF-1.4\n% e2e drawing\n%%EOF\n";
  writeFileSync(drawing, pdf);
  await (await b.$("input[type='file']")).setValue(drawing);
  await (await b.$("//td[normalize-space(.)='drawing-line3.pdf']")).waitForExist({ timeout: 15000 });
  check("requester attaches a PDF to the request", true);
  // Second item from pasted text through the AI panel (offline mock provider).
  await (await b.$("//summary[normalize-space(.)='Fill items from an email or list (AI)']")).click();
  await (await field("Text", "textarea")).setValue("Also please add: 4 set THK HSR15 linear guide");
  await click("Suggest items");
  // Low-confidence suggestions are never pre-ticked: the requester ticks it after checking.
  const box = await b.$("//details[@open]//input[@type='checkbox']");
  await box.waitForExist({ timeout: 15000 });
  check("AI suggestion is not pre-selected for the requester", !(await box.isSelected()));
  await box.click();
  await click("Add 1 selected");
  await b.waitUntil(async () => (await b.$$("//td[normalize-space(.)='HSR15']")).length === 1, {
    timeout: 15000,
    timeoutMsg: "AI-suggested item not added",
  });
  check("requester adds an AI-suggested item only after ticking it", true);
  await click("Submit to Chuo-Sogo");
  check("requester submits it", await headerStatus("Submitted"));
  await shot("01-requester-submitted");
  await signOut();

  // --- Operator ----------------------------------------------------------------------
  await signIn("operator@chuo-sogo.e2e");
  // The work queue shows the automatic "identify" task and opens the request in its org.
  await nav("Operations queue");
  const taskRow = await b.$(`//tr[.//button[normalize-space(.)='${number}']]`);
  await taskRow.waitForExist({ timeout: 15000 });
  check(
    "submitting created an identify task in the operator's queue",
    (await taskRow.getText()).includes("Identify products"),
  );
  await click(number);
  check(
    "operator switches into the assigned customer org",
    (await (await b.$(".brand-workspace")).getText()) === "Serving customer",
  );
  // An internal note and a shared comment.
  for (const [visibility, text] of [
    ["Internal (Chuo-Sogo only)", "INTERNAL-OPS-NOTE margin is thin"],
    ["Customer and Chuo-Sogo", "We are identifying the parts now."],
  ]) {
    await (await field("Add a comment", "textarea")).setValue(text);
    await (await field("Visible to", "select")).selectByVisibleText(visibility);
    await click("Send");
    await (await b.$(`//li//div[contains(., '${text.slice(0, 20)}')]`)).waitForExist({ timeout: 15000 });
  }
  await click("Pick up");
  check("operator picks the request up", await headerStatus("Identifying products"));
  // No catalogue records yet: create each with evidence, verified in the same step.
  for (const [row, url] of [
    [1, "https://www.nsk.example/6204ZZ-datasheet.pdf"],
    [2, "https://www.thk.example/HSR15.pdf"],
  ]) {
    // Wait until the previous verification panel has closed and the table is stable.
    await b.waitUntil(async () => (await b.$$(buttonXpath("Confirm verified"))).length === 0, {
      timeout: 15000,
    });
    const rowXpath = `//table//tr[td[1][normalize-space(.)='${row}']]`;
    // Click through the DOM: rows change height as evidence appears, so a coordinate click
    // could land on the row above.
    const verifyButton = await b.$(`${rowXpath}//button[normalize-space(.)='Verify']`);
    await verifyButton.waitForExist({ timeout: 15000 });
    await b.execute((el) => el.click(), verifyButton);
    await b.waitUntil(
      async () =>
        (await b.$$(`${rowXpath}/following-sibling::tr[1]//button[normalize-space(.)='Create a new record']`))
          .length === 1,
      { timeout: 15000, timeoutMsg: `verification panel for row ${row} did not open` },
    );
    await click("Create a new record");
    await (await field("Source (https)")).setValue(url);
    await click("Create and verify record");
    const radio = await b.$("input[type='radio']");
    await radio.waitForExist({ timeout: 15000 });
    check(`record ${row} is verified and preselected`, await radio.isSelected());
    await click("Confirm verified");
    await b.waitUntil(async () => (await b.$$("//td//span[normalize-space(.)='Verified']")).length === row, {
      timeout: 15000,
    });
  }
  await click("Start sourcing");
  check("sourcing only after verification", await headerStatus("Sourcing"));

  for (const s of ["Tokyo Bearings KK", "Osaka Motion Ltd"]) {
    await (await b.$(`//label[contains(normalize-space(.), '${s}')]/input`)).click();
  }
  await click("Create RFQ");
  await click("Dispatch (test mode)");
  check("RFQ dispatched in test mode", await headerStatus("RFQ sent"));
  await click("Show messages");
  await b.waitUntil(async () => (await b.$$("details pre")).length === 2, { timeout: 15000 });
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

  // Tokyo: pre-filled by AI from the supplier's text, checked and saved by the operator.
  await click("Record quotation");
  await (await field("Supplier", "select")).selectByVisibleText("Tokyo Bearings KK");
  await (await b.$('//summary[normalize-space(.)="Read the supplier\'s text (AI)"]')).click();
  await (
    await field("Supplier's reply", "textarea")
  ).setValue(
    // Line breaks typed as Enter keys: WebDriver drops "\n" in plain text.
    [
      "Currency: JPY",
      "Line 1: 1,200 x 100, 7 days",
      "Line 2: 45,000 x 4, 14 days",
      "Shipping: 0",
      "Ref: TB-881",
    ].join("\uE007"),
  );
  await click("Pre-fill from text");
  await (await b.$("//p[contains(., 'Form pre-filled by AI')]")).waitForExist({ timeout: 15000 });
  const prices = await b.execute(() =>
    Array.from(document.querySelectorAll("input[aria-label='Unit price']")).map((e) => e.value),
  );
  check("AI pre-fills exact prices from the supplier text", prices.join("|") === "1200|45000");
  await click("Save");
  await b.waitUntil(async () => (await b.$$("input[aria-label='Unit price']")).length === 0, {
    timeout: 15000,
  });
  // Osaka: typed by hand.
  await click("Record quotation");
  await (await field("Supplier", "select")).selectByVisibleText("Osaka Motion Ltd");
  const priceInputs = await b.$$("input[aria-label='Unit price']");
  const leadInputs = await b.$$("input[aria-label='Lead time (days)']");
  for (const [i, price] of ["1300", "46000"].entries()) {
    await priceInputs[i].setValue(price);
    await leadInputs[i].setValue("10");
  }
  await click("Save");
  await b.waitUntil(async () => (await b.$$("input[aria-label='Unit price']")).length === 0, {
    timeout: 15000,
  });
  check("two quotations recorded", await headerStatus("Quotes received"));
  await click("Run comparison");
  check("comparison runs", await headerStatus("Under comparison"));
  const firstRow = await b.$("//h3[normalize-space(.)='Comparison']/following::table[1]//tbody/tr[1]");
  await firstRow.waitForExist({ timeout: 15000 });
  const top = await firstRow.getText();
  check(
    "cheapest compliant quotation ranks first (exact total ¥300,000)",
    top.includes("Tokyo Bearings") && top.includes("300,000"),
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
  // Comments load separately: wait for the shared one before reading the page.
  await (
    await b.$("//li//div[contains(., 'We are identifying the parts now.')]")
  ).waitForExist({ timeout: 15000 });
  const page = await (await b.$("main")).getText();
  check("customer never sees internal supplier notes", !page.includes("INTERNAL-NOTE-DO-NOT-SHOW"));
  check(
    "customer sees the shared comment but never the internal note",
    page.includes("We are identifying the parts now.") && !page.includes("INTERNAL-OPS-NOTE"),
  );
  await shot("04-manager-approval");
  await click("Save to Downloads");
  const status = await b.$("p[role='status']");
  await status.waitForExist({ timeout: 15000 });
  const savedTo = (await status.getText()).replace(/^Saved to /, "");
  check(
    "attachment saved to Downloads by the native layer, byte for byte",
    existsSync(savedTo) && readFileSync(savedTo, "utf8") === pdf && savedTo.includes("/Downloads/"),
  );
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
