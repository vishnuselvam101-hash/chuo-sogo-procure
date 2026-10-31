// Desktop end-to-end smoke test against the real release binary, via tauri-driver
// (WebKitWebDriver underneath). Run with scripts/e2e-desktop.sh, which starts the
// backend, a virtual display and tauri-driver with an isolated config directory.
//   E2E_APP          path to the built procure-desktop binary
//   E2E_EMAIL/E2E_PASSWORD  an active platform user
//   E2E_OUT          directory for screenshots
import { remote } from "webdriverio";

const { E2E_APP, E2E_EMAIL, E2E_PASSWORD, E2E_OUT } = process.env;
const results = [];
function check(name, cond) {
  results.push({ name, pass: Boolean(cond) });
  console.log(`${cond ? "PASS" : "FAIL"} ${name}`);
}

async function session() {
  return remote({
    hostname: "127.0.0.1",
    port: 4444,
    logLevel: "error",
    capabilities: { "tauri:options": { application: E2E_APP } },
  });
}

async function signIn(b) {
  await (await b.$('input[type="email"]')).setValue(E2E_EMAIL);
  await (await b.$('input[type="password"]')).setValue(E2E_PASSWORD);
  await (await b.$('button[type="submit"]')).click();
}

async function navLabels(b) {
  const nav = await b.$(".nav-list");
  await nav.waitForExist({ timeout: 15000 });
  return nav.getText();
}

let b = await session();
try {
  const title = await (await b.$("h1")).getText();
  check("app launches and shows its own identity", title === "Chuo-Sogo Procure");
  await b.saveScreenshot(`${E2E_OUT}/01-login.png`);

  await signIn(b);
  const ja = await navLabels(b);
  check("authenticates against the backend and shows Japanese navigation", ja.includes("ダッシュボード"));
  check(
    "internal operations workspace for platform staff",
    (await (await b.$(".brand-workspace")).getText()) === "調達オペレーション",
  );
  await b.saveScreenshot(`${E2E_OUT}/02-dashboard-ja.png`);

  await (await b.$(".language-menu > button")).click();
  await (await b.$('button[role="option"][lang="ta"]')).click();
  check("language switches immediately to Tamil", (await navLabels(b)).includes("முகப்புப் பலகை"));
  await b.saveScreenshot(`${E2E_OUT}/03-dashboard-ta.png`);
} finally {
  await b.deleteSession(); // closes the app
}

// Restart: the preference must survive.
b = await session();
try {
  await signIn(b);
  check("Tamil choice persists across restart", (await navLabels(b)).includes("முகப்புப் பலகை"));
  await (await b.$(".language-menu > button")).click();
  await (await b.$('button[role="option"][lang="hi"]')).click();
  check("Hindi renders", (await navLabels(b)).includes("डैशबोर्ड"));
  await b.saveScreenshot(`${E2E_OUT}/04-dashboard-hi.png`);
  // Leave the environment in Japanese for the next run.
  await (await b.$(".language-menu > button")).click();
  await (await b.$('button[role="option"][lang="ja"]')).click();
} finally {
  await b.deleteSession();
}

const failed = results.filter((r) => !r.pass).length;
console.log(`${results.length - failed}/${results.length} passed`);
process.exit(failed ? 1 : 0);
