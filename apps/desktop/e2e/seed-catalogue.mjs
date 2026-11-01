// Seeds a throwaway backend for e2e/catalogue.mjs through the public API only: the bootstrap
// super administrator, one supplier organisation and one customer organisation, each with an
// active administrator. Prints the seeded identities as JSON on stdout.
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
for (const [name, kind, email] of [
  ["大阪ベアリング株式会社", "supplier", "sales@bearing.e2e"],
  ["株式会社イーツーイー工業", "customer", "buyer@customer.e2e"],
]) {
  const { org_id } = await call("POST", "/platform/organisations", root, {
    name,
    kind,
    admin_email: email,
    admin_name: email.split("@")[0],
  });
  const session = await activate(email);
  await call("POST", `/invitations/${org_id}/accept`, session);
}

console.log(
  JSON.stringify({
    password: PASSWORD,
    root: "root@chuo-sogo.e2e",
    supplier: "sales@bearing.e2e",
    supplierName: "大阪ベアリング株式会社",
    customer: "buyer@customer.e2e",
  }),
);
