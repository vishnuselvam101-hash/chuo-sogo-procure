import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { adminNav } from "../src/admin/AdminApp";
import { AgentExceptionsAdmin } from "../src/admin/AgentExceptionsAdmin";
import { buildResources, initI18n } from "../src/i18n";
import {
  ACTION_LABEL,
  AGENTS,
  EXCEPTION_STATUSES,
  RUN_STATUSES,
  SEVERITIES,
  type AgentException,
} from "../src/lib/agents";
import { BridgeProvider } from "../src/lib/BridgeContext";
import type { Bridge } from "../src/lib/bridge";
import { parseScaled } from "../src/lib/format";
import { navFor } from "../src/lib/nav";
import type { MembershipView } from "../src/lib/types";
import { AgentConsole } from "../src/pages/AgentConsole";
import { AgentExceptions } from "../src/pages/AgentExceptions";
import { CostWork, toModelInput } from "../src/pages/CostWork";
import { InvoiceExtract } from "../src/pages/InvoiceExtract";
import { Integrations, importKindsFor } from "../src/pages/Integrations";
import { MatchTolerances } from "../src/pages/MatchTolerances";
import { Scorecards } from "../src/pages/SupplierCompliance";

const ORG = "o1";
const BASE = `/api/v1/orgs/${ORG}`;

function member(permissions: string[], access = "member"): MembershipView {
  return {
    org_id: ORG,
    org_name: "A",
    org_kind: "customer",
    default_locale: "en",
    timezone: "Asia/Tokyo",
    role: "",
    permissions,
    access,
  } as MembershipView;
}

type Call = { method: string; path: string; body: unknown };
type Route = (method: string, path: string, body: unknown) => { status: number; body: unknown } | undefined;

/** A stand-in for the Rust layer that answers by method and path and records every call. */
function fakeBridge(route: Route) {
  const calls: Call[] = [];
  const api = vi.fn(async (method: string, path: string, body?: unknown) => {
    calls.push({ method, path, body: body ?? null });
    const r = route(method, path, body ?? null) ?? { status: 404, body: { error: { code: "not_found" } } };
    return { ...r, request_id: "r" };
  });
  return { bridge: { api, log: vi.fn() } as unknown as Bridge, calls };
}

function exception(over: Record<string, unknown>): AgentException {
  return {
    id: "e1",
    org_id: ORG,
    agent: "chasing",
    kind: "chase_due",
    severity: "warning",
    title_key: "chasing.due.order_ack",
    params: { po_number: "PO-7", supplier_name: "Tokyo Bearings", attempt: 1 },
    status: "open",
    dedupe_key: "k",
    decision_note: "",
    created_at: "2026-11-05T01:00:00Z",
    updated_at: "2026-11-05T01:00:00Z",
    proposed_action: null,
    suggestion_id: null,
    subject_id: "order1",
    subject_type: "purchase_order",
    ...over,
  } as unknown as AgentException;
}

const CHASE = exception({
  proposed_action: {
    type: "send_chase",
    order_id: "order1",
    chase_type: "order_ack",
    attempt: 1,
    draft: { lang: "ja", subject: "ご注文の確認 PO-7", body: "いつもお世話になっております。" },
  },
});

const MATCH = exception({
  id: "e2",
  agent: "matching",
  kind: "within_tolerance",
  severity: "info",
  title_key: "matching.within_tolerance",
  params: {
    invoice_number: "INV-9",
    discrepancy_kind: "price_mismatch",
    expected: 1000,
    actual: 1010,
    difference_minor: 10,
    currency: "JPY",
    invoiced_quantity: 1,
  },
  proposed_action: { type: "resolve_discrepancy", discrepancy_id: "d1", resolution: "accepted" },
  subject_id: "d1",
  subject_type: "discrepancy",
});

beforeAll(async () => {
  await initI18n("en");
});
afterEach(cleanup);

describe("agent navigation", () => {
  const ids = (p: string[], access = "member") => navFor(member(p, access)).map((n) => n.id);

  it("shows the console and inbox only with view_agents", () => {
    expect(ids(["view_agents", "decide_agent_exceptions"])).toEqual(
      expect.arrayContaining(["exceptions", "agents"]),
    );
    expect(ids(["submit_request", "view_own_requests"])).not.toContain("agents");
    expect(ids(["submit_request", "view_own_requests"])).not.toContain("exceptions");
  });

  it("gates cost work and data import by the permissions their APIs need", () => {
    expect(ids(["manage_purchase_history"])).toContain("cost");
    expect(ids(["view_finance"])).not.toContain("cost");
    expect(ids(["manage_invoices"])).toContain("integrations");
    expect(ids(["receive_goods"])).toContain("integrations");
    expect(ids(["view_agents"])).not.toContain("integrations");
    expect(importKindsFor(member(["receive_goods", "manage_invoices"]))).toEqual(["receipt", "invoice"]);
  });

  it("shows the staff exception view to procurement staff in the admin console", () => {
    expect(adminNav(["manage_suppliers", "manage_products"])).toContain("agents");
    expect(adminNav(["manage_organisations", "view_audit"])).not.toContain("agents");
  });
});

describe("agent console", () => {
  const summary = {
    enabled: true,
    runs_by_status: [{ key: "succeeded", count: 12 }],
    open_exceptions_by_severity: [{ key: "critical", count: 2 }],
    model_calls_this_month: 340,
    monthly_model_call_budget: 2000,
  };
  const settings = { enabled: true, auto_send_supplier_mail: false, monthly_model_call_budget: 2000 };
  const cadence = {
    ack_days: 2,
    delivery_date_days: 3,
    overdue_days: 1,
    invoice_days: 5,
    repeat_days: 2,
    max_attempts: 3,
  };
  const run = {
    id: "r1",
    org_id: ORG,
    agent: "chasing",
    trigger: "schedule",
    status: "succeeded",
    attempts: 1,
    run_after: "2026-11-05T00:00:00Z",
    input: {},
    model_calls: 0,
    created_at: "2026-11-05T00:00:00Z",
    finished_at: "2026-11-05T00:01:00Z",
  };
  const route: Route = (m, p, body) => {
    if (p === `${BASE}/agents/summary`) return { status: 200, body: summary };
    if (p.startsWith(`${BASE}/agents/runs`)) return { status: 200, body: [run] };
    if (p === `${BASE}/agents/settings`) return { status: 200, body: m === "PUT" ? body : settings };
    if (p === `${BASE}/agents/chasing/settings`) return { status: 200, body: cadence };
    if (m === "POST" && p.endsWith("/run")) return { status: 202, body: { id: "r2" } };
    return undefined;
  };

  it("shows counts and runs, starts a sweep and saves settings for an administrator", async () => {
    const { bridge, calls } = fakeBridge(route);
    render(
      <BridgeProvider value={bridge}>
        <AgentConsole active={member(["view_agents", "manage_agents", "decide_agent_exceptions"])} />
      </BridgeProvider>,
    );
    const stats = await screen.findByTestId("summary");
    expect(stats.textContent).toContain("Open: Urgent");
    expect(stats.textContent).toContain("340 / 2,000");
    expect(within(await screen.findByTestId("runs")).getByText("Supplier follow-up")).toBeTruthy();

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Run Invoice matching now" }));
    });
    expect(calls.some((c) => c.method === "POST" && c.path === `${BASE}/agents/matching/run`)).toBe(true);
    expect(screen.getByText("Invoice matching has been queued.")).toBeTruthy();

    const box = within(screen.getByTestId("agent-settings"));
    expect(box.getByText(/is emailed to the supplier straight away/)).toBeTruthy();
    fireEvent.click(box.getByLabelText("Send supplier email from approved actions automatically"));
    fireEvent.change(box.getByLabelText("Monthly AI call budget"), { target: { value: "500" } });
    await act(async () => {
      fireEvent.click(box.getByRole("button", { name: "Save" }));
    });
    const put = calls.find((c) => c.method === "PUT" && c.path === `${BASE}/agents/settings`);
    expect(put?.body).toEqual({
      enabled: true,
      auto_send_supplier_mail: true,
      monthly_model_call_budget: 500,
    });
  });

  it("is read only without manage_agents: no run buttons, no save, no PUT", async () => {
    const { bridge, calls } = fakeBridge(route);
    render(
      <BridgeProvider value={bridge}>
        <AgentConsole active={member(["view_agents", "decide_agent_exceptions"])} />
      </BridgeProvider>,
    );
    await screen.findByTestId("runs");
    const box = await screen.findByTestId("agent-settings");
    expect(screen.queryByTestId("run-now")).toBeNull();
    expect(within(box).queryByRole("button", { name: "Save" })).toBeNull();
    expect((box as HTMLFieldSetElement).disabled).toBe(true);
    expect(calls.every((c) => c.method === "GET")).toBe(true);
  });
});

describe("exception inbox", () => {
  it("renders titles from their locale key, previews the email and approves an edited draft", async () => {
    const { bridge, calls } = fakeBridge((m, p) => {
      if (m === "GET" && p.startsWith(`${BASE}/agents/exceptions`))
        return { status: 200, body: [CHASE, MATCH] };
      if (m === "POST" && p.endsWith("/decide")) return { status: 200, body: { ...CHASE, status: "edited" } };
      return undefined;
    });
    render(
      <BridgeProvider value={bridge}>
        <AgentExceptions active={member(["view_agents", "decide_agent_exceptions"])} />
      </BridgeProvider>,
    );
    const table = await screen.findByTestId("exceptions");
    expect(table.textContent).toContain("PO-7: Tokyo Bearings has not acknowledged the order (follow-up 1)");
    fireEvent.click(within(table).getAllByRole("button", { name: "Open" })[0] as HTMLElement);
    const detail = screen.getByTestId("detail");
    expect(within(detail).getByTestId("email-preview").textContent).toContain(
      "いつもお世話になっております。",
    );

    fireEvent.click(within(detail).getByRole("button", { name: "Edit" }));
    fireEvent.change(within(detail).getByLabelText("Message"), {
      target: { value: "改めてご確認ください。" },
    });
    await act(async () => {
      fireEvent.click(within(detail).getByRole("button", { name: "Approve with changes" }));
    });
    const decide = calls.find((c) => c.method === "POST");
    expect(decide?.path).toBe(`${BASE}/agents/exceptions/e1/decide`);
    expect(decide?.body).toMatchObject({
      decision: "edit",
      action: {
        type: "send_chase",
        order_id: "order1",
        attempt: 1,
        draft: { body: "改めてご確認ください。" },
      },
    });
  });

  it("needs a note to reject and filters by severity", async () => {
    const { bridge, calls } = fakeBridge((m, p) =>
      m === "GET" && p.startsWith(`${BASE}/agents/exceptions`)
        ? { status: 200, body: [CHASE, MATCH] }
        : undefined,
    );
    render(
      <BridgeProvider value={bridge}>
        <AgentExceptions active={member(["view_agents", "decide_agent_exceptions"])} />
      </BridgeProvider>,
    );
    const table = await screen.findByTestId("exceptions");
    fireEvent.change(screen.getByLabelText("Severity"), { target: { value: "info" } });
    expect(within(table).getAllByRole("row")).toHaveLength(2);
    expect(table.textContent).toContain("INV-9");
    fireEvent.click(within(table).getByRole("button", { name: "Open" }));
    const detail = screen.getByTestId("detail");
    expect(detail.textContent).toContain("Unit price differs");
    expect(within(detail).queryByRole("button", { name: "Edit" })).toBeTruthy();
    fireEvent.click(within(detail).getByRole("button", { name: "Reject" }));
    expect(calls.some((c) => c.method === "POST")).toBe(false);
    expect(within(detail).getByRole("alert")).toBeTruthy();
  });

  it("only shows the decision buttons with decide_agent_exceptions", async () => {
    const { bridge } = fakeBridge((m, p) =>
      m === "GET" && p.startsWith(`${BASE}/agents/exceptions`) ? { status: 200, body: [CHASE] } : undefined,
    );
    render(
      <BridgeProvider value={bridge}>
        <AgentExceptions active={member(["view_agents"])} />
      </BridgeProvider>,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Open" }));
    const detail = screen.getByTestId("detail");
    expect(within(detail).queryByRole("button", { name: "Approve" })).toBeNull();
    expect(within(detail).queryByRole("button", { name: "Reject" })).toBeNull();
  });

  it("shows the linked AI reading of a supplier reply", async () => {
    const reply = exception({
      id: "e3",
      title_key: "chasing.reply.confirmed",
      params: {
        po_number: "PO-7",
        supplier_name: "Tokyo Bearings",
        promised_date: "2026-11-20",
        acknowledged: "yes",
      },
      suggestion_id: "s1",
      proposed_action: { type: "record_promised_date", order_id: "order1", date: "2026-11-20" },
    });
    const { bridge } = fakeBridge((m, p) => {
      if (p.startsWith(`${BASE}/agents/exceptions`)) return { status: 200, body: [reply] };
      if (p === `${BASE}/ai/suggestions/s1`)
        return {
          status: 200,
          body: { id: "s1", status: "pending", model: "m", output: { questions: ["分納は可能ですか"] } },
        };
      return undefined;
    });
    render(
      <BridgeProvider value={bridge}>
        <AgentExceptions active={member(["view_agents", "decide_agent_exceptions"])} />
      </BridgeProvider>,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Open" }));
    expect((await screen.findByTestId("suggestion")).textContent).toContain("分納は可能ですか");
    expect(screen.getByTestId("proposed").textContent).toContain("Record acknowledgement and date");
  });

  it("lists assigned customers' exceptions read only for staff", async () => {
    const { bridge } = fakeBridge((m, p) =>
      p.startsWith("/api/admin/v1/ops/agents/exceptions")
        ? { status: 200, body: [{ org_name: "株式会社エー製作所", exception: CHASE }] }
        : undefined,
    );
    render(
      <BridgeProvider value={bridge}>
        <AgentExceptionsAdmin />
      </BridgeProvider>,
    );
    const table = await screen.findByTestId("staff-exceptions");
    expect(table.textContent).toContain("株式会社エー製作所");
    fireEvent.click(within(table).getByRole("button", { name: "Open" }));
    expect(within(screen.getByTestId("detail")).queryByRole("button", { name: "Approve" })).toBeNull();
  });
});

describe("matching tolerances and invoice extraction", () => {
  const tol = {
    currency: "JPY",
    price_bps: 0,
    price_line_abs_minor: 0,
    quantity_bps: 0,
    quantity_abs: 0,
    rounding_minor: 0,
  };

  it("saves percentages and amounts exactly", async () => {
    const { bridge, calls } = fakeBridge((m, p, body) =>
      p === `${BASE}/matching/tolerances` ? { status: 200, body: m === "PUT" ? body : tol } : undefined,
    );
    render(
      <BridgeProvider value={bridge}>
        <MatchTolerances active={member(["view_orders", "manage_invoices"])} />
      </BridgeProvider>,
    );
    fireEvent.change(await screen.findByLabelText("Unit price tolerance (%)"), {
      target: { value: "1.5" },
    });
    fireEvent.change(screen.getByLabelText("Maximum difference per line"), { target: { value: "500" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
    });
    expect(calls.find((c) => c.method === "PUT")?.body).toEqual({
      ...tol,
      price_bps: 150,
      price_line_abs_minor: 500,
    });
  });

  it("is read only without manage_invoices", async () => {
    const { bridge } = fakeBridge((_m, p) =>
      p === `${BASE}/matching/tolerances` ? { status: 200, body: tol } : undefined,
    );
    render(
      <BridgeProvider value={bridge}>
        <MatchTolerances active={member(["view_orders"])} />
      </BridgeProvider>,
    );
    const box = await screen.findByTestId("tolerances");
    expect((box as HTMLFieldSetElement).disabled).toBe(true);
    expect(within(box).queryByRole("button", { name: "Save" })).toBeNull();
  });

  it("hands the AI reading to the invoice form without recording anything", async () => {
    const output = {
      supplier_invoice_number: "INV-1",
      currency: "JPY",
      shipping_minor: 0,
      tax_rate_bps: 1000,
      tax_minor: 100,
      lines: [],
    };
    const { bridge, calls } = fakeBridge(() => ({
      status: 201,
      body: { id: "s", status: "pending", output },
    }));
    const onSuggestion = vi.fn();
    render(
      <BridgeProvider value={bridge}>
        <InvoiceExtract orgId={ORG} orderId="po1" onSuggestion={onSuggestion} />
      </BridgeProvider>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Read an invoice" }));
    fireEvent.change(screen.getByLabelText("Invoice text"), { target: { value: "請求書 INV-1" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Read with AI" }));
    });
    expect(calls.map((c) => c.path)).toEqual([`${BASE}/orders/po1/ai/extract-invoice`]);
    expect(onSuggestion).toHaveBeenCalledWith(output);
  });
});

describe("cost work", () => {
  it("turns the model form into exact integer inputs", () => {
    expect(parseScaled("0.25", 3)).toBe(250);
    expect(parseScaled("1.2345", 3)).toBeNull();
    const body = toModelInput({
      name: "Bracket",
      mpn: "BR-1",
      description: "",
      currency: "JPY",
      lot: "100",
      overhead: "10",
      margin: "8.5",
      logistics: "3000",
      notes: "",
      materials: [{ name: "SPCC", unit: "kg", qty: "0.25", price: "180", scrap: "5" }],
      processes: [{ name: "Press", cycle: "4.5", rate: "6000", setup: "30" }],
    });
    expect(body?.inputs).toEqual({
      lot_size: 100,
      overhead_bps: 1000,
      margin_bps: 850,
      logistics_minor_per_lot: 3000,
      materials: [{ name: "SPCC", unit: "kg", quantity_milli: 250, unit_price_minor: 180, scrap_bps: 500 }],
      processes: [{ name: "Press", cycle_ms: 4500, rate_minor_per_hour: 6000, setup_minutes: 30 }],
    });
  });

  it("hides model creation without manage_purchase_history", async () => {
    const { bridge } = fakeBridge((_m, p) =>
      p.startsWith(`${BASE}/cost/`) ? { status: 200, body: [] } : undefined,
    );
    render(
      <BridgeProvider value={bridge}>
        <CostWork active={member(["view_all_requests"])} />
      </BridgeProvider>,
    );
    fireEvent.click(screen.getByRole("tab", { name: "Cost models" }));
    await screen.findByText(/No cost models yet/);
    expect(screen.queryByRole("button", { name: "New cost model" })).toBeNull();
  });
});

describe("data import", () => {
  const batch = {
    id: "b1",
    kind: "invoice",
    file_name: "invoices.csv",
    file_sha256: "x",
    profile: "",
    currency: "JPY",
    mapping: [],
    status: "previewed",
    data_rows: 3,
    records: 1,
    error_rows: 1,
    errors: [{ row: 3, code: "date_invalid", field: "invoice_date" }],
    created_by: "u",
    created_at: "2026-11-05T00:00:00Z",
    staged: [
      { kind: "invoice", po_number: "PO-7", invoice_number: "INV-1", currency: "JPY", lines: [], rows: [2] },
    ],
  };
  const ex = exception({
    id: "e9",
    agent: "import",
    kind: "import_ready",
    severity: "info",
    title_key: "integrations.import_ready",
    params: { file_name: "invoices.csv", records: 1, kind: "invoice" },
    proposed_action: { type: "apply_import", batch_id: "b1" },
    subject_id: "b1",
    subject_type: "import_batch",
  });

  it("uploads, previews row errors and applies through the exception", async () => {
    const { bridge, calls } = fakeBridge((m, p) => {
      if (p === `${BASE}/integrations/imports` && m === "POST") return { status: 201, body: batch };
      if (p.startsWith(`${BASE}/integrations/imports`))
        return { status: 200, body: m === "GET" && p.endsWith("b1") ? batch : [] };
      if (p === `${BASE}/integrations/profiles`) return { status: 200, body: [] };
      if (p.startsWith(`${BASE}/agents/exceptions?`)) return { status: 200, body: [ex] };
      if (p.endsWith("/decide")) return { status: 200, body: { ...ex, status: "approved" } };
      return undefined;
    });
    render(
      <BridgeProvider value={bridge}>
        <Integrations active={member(["manage_invoices", "view_agents", "decide_agent_exceptions"])} />
      </BridgeProvider>,
    );
    const file = new File(["po,inv\n"], "invoices.csv", { type: "text/csv" });
    fireEvent.change(await screen.findByLabelText(/^File/), { target: { files: [file] } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check file" }));
    });
    await waitFor(() => expect(calls.some((c) => c.method === "POST")).toBe(true));
    expect((calls.find((c) => c.method === "POST")?.body as { kind: string }).kind).toBe("invoice");
    expect((await screen.findByTestId("row-errors")).textContent).toContain("Unreadable date");
    const detail = await screen.findByTestId("detail");
    await act(async () => {
      fireEvent.click(within(detail).getByRole("button", { name: "Approve" }));
    });
    expect(calls.some((c) => c.path === `${BASE}/agents/exceptions/e9/decide`)).toBe(true);
    expect(calls.some((c) => c.path.endsWith("/apply"))).toBe(false);
  });
});

describe("supplier scorecards", () => {
  it("shows percentages and says when data is missing", async () => {
    const { bridge, calls } = fakeBridge(() => ({
      status: 200,
      body: [
        {
          supplier_id: "s1",
          supplier_name: "Tokyo Bearings",
          month: "2026-10",
          deliveries_measured: 4,
          invoices: 2,
          rfqs_received: 3,
          comparisons_ranked: 1,
          on_time_bps: 7500,
          discrepancy_bps: null,
        },
      ],
    }));
    render(
      <BridgeProvider value={bridge}>
        <Scorecards active={member(["view_agents"])} />
      </BridgeProvider>,
    );
    const box = await screen.findByTestId("scorecards");
    await within(box).findByText("Tokyo Bearings");
    expect(box.textContent).toContain("75%");
    expect(box.textContent).toContain("Not enough data");
    expect(calls[0]?.path).toMatch(/supplier-scorecards\?month=\d{4}-\d{2}$/);
  });
});

describe("agent locale keys", () => {
  const resources = buildResources();
  const get = (lang: string, path: string) =>
    path.split(".").reduce<unknown>((o, k) => (o as Record<string, unknown>)?.[k], resources[lang]?.agents);

  it("label every agent, status, severity and action in ja, en, ta and hi", () => {
    const keys = [
      ...AGENTS.map((a) => `ui.agent.${a}`),
      ...SEVERITIES.map((s) => `runtime.severity.${s}`),
      ...RUN_STATUSES.map((s) => `runtime.runStatus.${s}`),
      ...EXCEPTION_STATUSES.map((s) => `runtime.exceptionStatus.${s}`),
      ...Object.values(ACTION_LABEL),
      // Title keys the server's agents raise.
      "matching.within_tolerance",
      "matching.out_of_tolerance",
      "matching.needs_review",
      "chasing.due.order_ack",
      "chasing.exhausted.missing_invoice",
      "chasing.reply.suspicious",
      "cost.negotiationOpportunity.title",
      "cost.negotiationOpportunity.body",
      "supplier.documentExpiring",
      "supplier.documentExpired",
      "supplier.scorecardReview",
      "integrations.import_ready",
      "integrations.import_blocked",
    ];
    for (const lang of ["ja", "en", "ta", "hi"])
      for (const k of keys) expect(typeof get(lang, k), `${lang}:${k}`).toBe("string");
  });

  it("is primarily Japanese", async () => {
    const i18n = await initI18n("ja");
    expect(i18n.t("nav.exceptions")).toBe("例外対応");
    expect(i18n.t("agents:ui.detail.approve")).toBe("承認する");
    await i18n.changeLanguage("en");
  });
});
