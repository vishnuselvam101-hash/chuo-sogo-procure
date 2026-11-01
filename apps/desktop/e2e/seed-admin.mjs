// Seeds a throwaway backend for e2e/admin.mjs through the public API only: activates the
// bootstrap super administrator, adds one Chuo-Sogo operator and one customer organisation.
// Prints the seeded identities as JSON on stdout.
//   E2E_API, E2E_OUTBOX, E2E_ROOT_TOKEN: as for seed.mjs
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const { E2E_API, E2E_OUTBOX, E2E_ROOT_TOKEN } = process.env;
const PASSWORD = "e2e-correct-horse-battery";

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
  for (const f of readdirSync(E2E_OUTBOX).sort().reverse()) {
    const text = readFileSync(join(E2E_OUTBOX, f), "utf8");
    if (text.includes(`To: ${email}\n`)) return /^token: (\S+)$/m.exec(text)[1];
  }
  throw new Error(`no mail for ${email}`);
}

async function activate(email, token = mailedToken(email)) {
  await call("POST", "/auth/set-password", null, { token, password: PASSWORD });
  return (await call("POST", "/auth/login", null, { email, password: PASSWORD })).token;
}

const root = await activate("root@chuo-sogo.e2e", E2E_ROOT_TOKEN);
const platform = (await call("GET", "/me", root)).memberships[0].org_id;
await call("POST", `/orgs/${platform}/members`, root, {
  email: "operator@chuo-sogo.e2e",
  display_name: "Operator One",
  role: "procurement_operator",
});
const operator = await activate("operator@chuo-sogo.e2e");
await call("POST", `/invitations/${platform}/accept`, operator);
const ORG_NAME = "株式会社アドミン検証";
await call("POST", "/platform/organisations", root, {
  name: ORG_NAME,
  admin_email: "admin@customer.e2e",
  admin_name: "Customer Admin",
  default_locale: "ja",
});

console.log(
  JSON.stringify({ password: PASSWORD, root: "root@chuo-sogo.e2e", operator: "Operator One", org: ORG_NAME }),
);
