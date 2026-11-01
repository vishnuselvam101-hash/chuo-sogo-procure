// Seeds a throwaway backend for e2e/workflow.mjs through the public API only.
// Account activation tokens are read from the test-mode mail outbox (files), exactly as a
// developer would. Prints the seeded identities as JSON on stdout.
//   E2E_API      backend base URL (http://127.0.0.1:8088)
//   E2E_OUTBOX   the server's PROCURE_OUTBOX_DIR
//   E2E_ROOT_TOKEN  activation token printed by `procure-server bootstrap`
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const { E2E_API, E2E_OUTBOX, E2E_ROOT_TOKEN } = process.env;
export const PASSWORD = "e2e-correct-horse-battery";

async function call(method, path, token, body) {
  const res = await fetch(`${E2E_API}/api/v1${path}`, {
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

function mailedToken(email) {
  const files = readdirSync(E2E_OUTBOX).sort().reverse();
  for (const f of files) {
    const text = readFileSync(join(E2E_OUTBOX, f), "utf8");
    if (text.includes(`To: ${email}\n`)) return /^token: (\S+)$/m.exec(text)[1];
  }
  throw new Error(`no mail for ${email}`);
}

async function activate(email, token = mailedToken(email)) {
  await call("POST", "/auth/set-password", null, { token, password: PASSWORD });
  return (await call("POST", "/auth/login", null, { email, password: PASSWORD })).token;
}

async function member(org, admin, email, role) {
  await call("POST", `/orgs/${org}/members`, admin, { email, display_name: email.split("@")[0], role });
  const session = await activate(email);
  await call("POST", `/invitations/${org}/accept`, session);
  return session;
}

const root = await activate("root@chuo-sogo.e2e", E2E_ROOT_TOKEN);
const platform = (await call("GET", "/me", root)).memberships[0].org_id;
const ORG_NAME = "株式会社イーツーイー工業";
const { org_id: org } = await call("POST", "/platform/organisations", root, {
  name: ORG_NAME,
  admin_email: "admin@customer.e2e",
  admin_name: "Customer Admin",
  default_locale: "ja",
});
const admin = await activate("admin@customer.e2e");
await call("POST", `/invitations/${org}/accept`, admin);

const operator = await member(platform, root, "operator@chuo-sogo.e2e", "procurement_operator");
const operatorId = (await call("GET", "/me", operator)).user.id;
await call("POST", `/orgs/${org}/assignments`, root, { user_id: operatorId });
await member(org, admin, "requester@customer.e2e", "requester");
const manager = await member(org, admin, "manager@customer.e2e", "procurement_manager");

for (const [name, email] of [
  ["Tokyo Bearings KK", "rfq@tokyo-bearings.e2e"],
  ["Osaka Motion Ltd", "sales@osaka-motion.e2e"],
]) {
  const { id } = await call("POST", "/suppliers", operator, {
    name,
    country: "JP",
    email,
    categories: ["bearings"],
    notes: "INTERNAL-NOTE-DO-NOT-SHOW",
  });
  await call("POST", `/suppliers/${id}/status`, operator, { status: "verified" });
  await call("POST", `/orgs/${org}/suppliers/${id}/proposal`, operator);
  await call("PUT", `/orgs/${org}/suppliers/${id}/decision`, manager, { decision: "approved" });
}

console.log(JSON.stringify({ org, orgName: ORG_NAME, password: PASSWORD }));
