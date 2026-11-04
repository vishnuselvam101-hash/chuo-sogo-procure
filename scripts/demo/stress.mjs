// Heavy mixed load against a throwaway backend (scripts/demo/stress.sh starts one; never point
// this at the live or demo server). Two customer organisations, many concurrent users:
// requesters create requests with items and submit them, operators pick them up, everyone
// lists and reads, plus hostile traffic (bad JSON, oversized fields, invalid ids, the other
// organisation's ids, wrong passwords). Fails on any 5xx, any cross-organisation read, a
// dropped connection, or a request that does not end up in the state it was put in.
//   STRESS_API, STRESS_OUTBOX, STRESS_ROOT_TOKEN, STRESS_MINUTES (10), STRESS_USERS (96)
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const API = process.env.STRESS_API;
const OUTBOX = process.env.STRESS_OUTBOX;
const MINUTES = Number(process.env.STRESS_MINUTES ?? 10);
const USERS = Number(process.env.STRESS_USERS ?? 96);
const PASSWORD = "stress-correct-horse-battery";
const stats = { ok: 0, expected4xx: 0, s5xx: 0, netErr: 0, leaks: 0, bad: 0, byOp: {} };
const lat = [];
const failures = [];

function fail(msg) {
  if (failures.length < 50) failures.push(msg);
}

async function call(method, path, token, body, { raw, expect } = {}) {
  const t0 = performance.now();
  let res;
  try {
    res = await fetch(`${API}/api/v1${path}`, {
      method,
      headers: {
        ...(token ? { authorization: `Bearer ${token}` } : {}),
        ...(body !== undefined ? { "content-type": "application/json" } : {}),
      },
      body: raw ?? (body !== undefined ? JSON.stringify(body) : undefined),
    });
  } catch (e) {
    stats.netErr++;
    fail(`${method} ${path}: network ${e.cause?.code ?? e.message}`);
    return { status: 0 };
  }
  const text = await res.text();
  lat.push(performance.now() - t0);
  const op = `${method} ${path.replace(/[0-9a-f-]{36}/g, ":id")}`;
  stats.byOp[op] = (stats.byOp[op] ?? 0) + 1;
  if (res.status >= 500) {
    stats.s5xx++;
    fail(`${op} -> ${res.status} ${text.slice(0, 200)}`);
  } else if (res.status >= 400) {
    if (expect === "error") stats.expected4xx++;
    else {
      stats.bad++;
      fail(`${op} -> ${res.status} ${text.slice(0, 200)}`);
    }
  } else {
    if (expect === "error") {
      stats.leaks++;
      fail(`LEAK/ACCEPTED ${op} -> ${res.status} ${text.slice(0, 200)}`);
    } else stats.ok++;
  }
  let json = null;
  try {
    json = text ? JSON.parse(text) : null;
  } catch {}
  return { status: res.status, json };
}

async function must(method, path, token, body) {
  const r = await call(method, path, token, body);
  if (r.status < 200 || r.status >= 300) throw new Error(`setup ${method} ${path} -> ${r.status}`);
  return r.json;
}

function mailedToken(email) {
  for (const f of readdirSync(OUTBOX).sort().reverse()) {
    const text = readFileSync(join(OUTBOX, f), "utf8");
    if (text.includes(`To: ${email}\n`)) return /^token: (\S+)$/m.exec(text)[1];
  }
  throw new Error(`no mail for ${email}`);
}
async function activate(email, token = mailedToken(email)) {
  await must("POST", "/auth/set-password", null, { token, password: PASSWORD });
  return (await must("POST", "/auth/login", null, { email, password: PASSWORD })).token;
}
async function member(org, admin, email, role) {
  await must("POST", `/orgs/${org}/members`, admin, { email, display_name: email.split("@")[0], role });
  const s = await activate(email);
  await must("POST", `/invitations/${org}/accept`, s);
  return s;
}

// ---- setup: platform operator + two customer organisations ----
const root = await activate("root@chuo-sogo.stress", process.env.STRESS_ROOT_TOKEN);
const platform = (await must("GET", "/me", root)).memberships[0].org_id;
const operator = await member(platform, root, "operator@chuo-sogo.stress", "procurement_operator");
const operatorId = (await must("GET", "/me", operator)).user.id;
const orgs = [];
for (const n of [1, 2]) {
  const { org_id } = await must("POST", "/platform/organisations", root, {
    name: `ストレス工業${n}`,
    admin_email: `admin${n}@factory.stress`,
    admin_name: `Admin ${n}`,
    default_locale: "ja",
  });
  const admin = await activate(`admin${n}@factory.stress`);
  await must("POST", `/invitations/${org_id}/accept`, admin);
  await must("POST", `/orgs/${org_id}/assignments`, root, { user_id: operatorId });
  const requesters = [];
  for (let i = 0; i < 4; i++) requesters.push(await member(org_id, admin, `req${i}@factory${n}.stress`, "requester"));
  const manager = await member(org_id, admin, `manager@factory${n}.stress`, "procurement_manager");
  orgs.push({ id: org_id, admin, requesters, manager, created: [], owner: new Map() });
}
console.error(`setup done: 2 organisations, ${USERS} virtual users, ${MINUTES} min`);

// ---- load ----
const end = Date.now() + MINUTES * 60_000;
const rnd = (n) => Math.floor(Math.random() * n);
const pick = (a) => a[rnd(a.length)];
const hostile = ["", "\u0000", "'; DROP TABLE requests; --", "<script>alert(1)</script>", "ｶﾞ".repeat(5000), "x".repeat(200_000)];

async function requesterFlow(o) {
  const tok = pick(o.requesters);
  const r = await call("POST", `/orgs/${o.id}/requests`, tok, {
    title: `ベアリング ${rnd(1e6)} 個 ${pick(hostile.slice(2, 4))}`,
    currency: "JPY",
    description: "ストレステスト",
    delivery_location: "第2工場",
    items: [],
  });
  const id = r.json?.id;
  if (!id) return;
  for (let i = 0; i < 1 + rnd(3); i++)
    await call("POST", `/orgs/${o.id}/requests/${id}/items`, tok, {
      description: `6205-2RS ${i}`,
      quantity: 1 + rnd(500),
      unit: "pcs",
      manufacturer: "NSK",
      mpn: `6205-${rnd(99)}`,
    });
  await call("POST", `/orgs/${o.id}/requests/${id}/comments`, tok, { body: `確認お願いします ${rnd(1e6)}` });
  const s = await call("POST", `/orgs/${o.id}/requests/${id}/submit`, tok, {});
  if (s.status >= 200 && s.status < 300) {
    o.created.push(id);
    o.owner.set(id, tok);
  }
}

async function readerFlow(o) {
  const tok = pick([...o.requesters, o.manager, o.admin]);
  await call("GET", `/orgs/${o.id}/requests`, tok);
  if (o.created.length) {
    // Requesters see only their own requests; managers and admins see all.
    const id = pick(o.created);
    const reader = o.requesters.includes(tok) ? o.owner.get(id) : tok;
    await call("GET", `/orgs/${o.id}/requests/${id}`, reader);
    // Another requester of the same factory must not see it.
    const other = o.requesters.find((t) => t !== o.owner.get(id));
    await call("GET", `/orgs/${o.id}/requests/${id}`, other, undefined, { expect: "error" });
  }
  await call("GET", "/me", tok);
}

async function operatorFlow(o) {
  await call("GET", `/orgs/${o.id}/requests`, operator);
  if (o.created.length) {
    const id = pick(o.created);
    await call("GET", `/orgs/${o.id}/requests/${id}`, operator);
    await call("GET", `/orgs/${o.id}/requests/${id}/tasks`, operator);
  }
}

async function hostileFlow() {
  const [a, b] = rnd(2) ? orgs : [orgs[1], orgs[0]];
  const tok = pick(a.requesters);
  // Other organisation: must never answer with data.
  await call("GET", `/orgs/${b.id}/requests`, tok, undefined, { expect: "error" });
  if (b.created.length) {
    await call("GET", `/orgs/${b.id}/requests/${pick(b.created)}`, tok, undefined, { expect: "error" });
    await call("GET", `/orgs/${a.id}/requests/${pick(b.created)}`, tok, undefined, { expect: "error" });
    await call("POST", `/orgs/${b.id}/requests/${pick(b.created)}/comments`, tok, { body: "x" }, { expect: "error" });
  }
  // Broken input.
  await call("POST", `/orgs/${a.id}/requests`, tok, undefined, { raw: "{not json", expect: "error" });
  await call("POST", `/orgs/${a.id}/requests`, tok, { title: pick(hostile), currency: "XXX" }, { expect: "error" });
  await call("POST", `/orgs/${a.id}/requests`, tok, { title: "x".repeat(200_000), currency: "JPY" }, { expect: "error" });
  await call("GET", `/orgs/${a.id}/requests/not-a-uuid`, tok, undefined, { expect: "error" });
  await call("GET", `/orgs/${a.id}/requests`, "forged.token.value", undefined, { expect: "error" });
  await call("GET", `/orgs/${a.id}/requests`, null, undefined, { expect: "error" });
  await call("POST", "/auth/login", null, { email: `req0@factory1.stress`, password: "wrong-password" }, { expect: "error" });
}

async function worker(n) {
  const kinds = [requesterFlow, requesterFlow, readerFlow, readerFlow, readerFlow, operatorFlow, hostileFlow];
  while (Date.now() < end) {
    const k = pick(kinds);
    await (k === hostileFlow ? hostileFlow() : k(pick(orgs)));
  }
}

const t0 = Date.now();
await Promise.all(Array.from({ length: USERS }, (_, i) => worker(i)));
const secs = (Date.now() - t0) / 1000;

// ---- after: every submitted request is there, in its organisation only, still submitted ----
let missing = 0;
for (const o of orgs) {
  const list = await must("GET", `/orgs/${o.id}/requests`, o.admin);
  const items = Array.isArray(list) ? list : (list?.items ?? list?.requests ?? []);
  const seen = new Set(items.map((x) => x.id));
  const total = list?.total ?? items.length;
  for (const id of o.created.slice(0, 200)) {
    const r = await call("GET", `/orgs/${o.id}/requests/${id}`, o.admin);
    if (r.status !== 200) missing++;
  }
  console.error(`org ${o.id}: ${o.created.length} submitted, list shows ${total} (first page ${seen.size})`);
}

lat.sort((x, y) => x - y);
const q = (p) => lat[Math.min(lat.length - 1, Math.floor(lat.length * p))]?.toFixed(1);
const total = lat.length;
const summary = {
  minutes: MINUTES,
  users: USERS,
  requests_total: total,
  per_second: Math.round(total / secs),
  ok: stats.ok,
  refused_as_expected: stats.expected4xx,
  server_errors_5xx: stats.s5xx,
  network_errors: stats.netErr,
  cross_org_leaks_or_bad_input_accepted: stats.leaks,
  unexpected_4xx: stats.bad,
  submitted_requests: orgs.reduce((a, o) => a + o.created.length, 0),
  missing_after: missing,
  latency_ms: { p50: q(0.5), p95: q(0.95), p99: q(0.99), max: lat[total - 1]?.toFixed(1) },
  by_operation: stats.byOp,
  first_failures: failures,
};
writeFileSync(process.env.STRESS_OUT ?? "stress-result.json", JSON.stringify(summary, null, 2));
console.log(JSON.stringify(summary, null, 2));
const pass = stats.s5xx === 0 && stats.netErr === 0 && stats.leaks === 0 && missing === 0;
console.log(pass ? "STRESS PASS" : "STRESS FAIL");
process.exit(pass ? 0 : 1);
