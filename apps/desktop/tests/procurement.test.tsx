import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { initI18n } from "../src/i18n";
import { BridgeProvider } from "../src/lib/BridgeContext";
import type { Bridge } from "../src/lib/bridge";
import { parseMinor, minorToInput } from "../src/lib/format";
import type { MembershipView, RecommendationDetail, RequestDetail as Detail } from "../src/lib/types";
import { ErrorText } from "../src/components/ui";
import { RequestDetail } from "../src/pages/RequestDetail";
import { NewRequest } from "../src/pages/NewRequest";
import { RfqPanel } from "../src/pages/RfqPanel";

const ORG = "0192f0aa-1111-7222-8333-444455556666";
const REQ = "0192f0aa-2222-7222-8333-444455556666";
const REC = "0192f0aa-3333-7222-8333-444455556666";
const ME = "user-1";

function view(access: "member" | "assigned", permissions: string[]): MembershipView {
  return {
    org_id: ORG,
    org_name: "株式会社エー製作所",
    org_kind: "customer",
    default_locale: "en",
    timezone: "Asia/Tokyo",
    role: access === "assigned" ? "procurement_operator" : "procurement_manager",
    permissions,
    access,
  };
}

function detail(status: string, recommendation = false): Detail {
  return {
    request: {
      id: REQ,
      number: "PR-2026-000001",
      title: "Bearings",
      description: "",
      status,
      currency: "JPY",
      needed_by: null,
      delivery_location: "",
      clarification_question: null,
      clarification_answer: null,
      requested_by: "someone-else",
      requester_name: "Req",
      created_at: "2026-11-01T00:00:00Z",
      updated_at: "2026-11-01T00:00:00Z",
      submitted_at: null,
    },
    items: [
      {
        id: "item-1",
        line_no: 1,
        description: "Ball bearing",
        manufacturer: "NSK",
        mpn: "6204ZZ",
        quantity: 100,
        unit: "pcs",
        notes: "",
        verification_status: "unverified",
        verified_manufacturer: null,
        verified_mpn: null,
        verification_evidence: null,
        verified_at: null,
        product_identity_id: null,
      },
    ],
    rfqs: [],
    recommendation: recommendation
      ? {
          id: REC,
          status: "awaiting_approval",
          currency: "JPY",
          total_minor: 302000,
          supplier_name: "Tokyo Bearings",
          required_approvals: 2,
          created_at: "2026-11-01T00:00:00Z",
        }
      : null,
  };
}

const rec: RecommendationDetail = {
  recommendation: {
    id: REC,
    request_id: REQ,
    request_number: "PR-2026-000001",
    requested_by: "someone-else",
    status: "awaiting_approval",
    currency: "JPY",
    total_minor: 302000,
    rank: 1,
    reason: "",
    quotation_id: "q1",
    supplier_id: "s1",
    supplier_name: "Tokyo Bearings",
    comparison_id: "c1",
    required_approvals: 2,
    policy_version: 1,
    created_at: "2026-11-01T00:00:00Z",
    sent_at: "2026-11-01T00:00:00Z",
    checked_at: "2026-11-01T00:00:00Z",
    checked_by_name: "Operator",
    check_note: "",
    decided_at: null,
  },
  comparison: [
    {
      quotation_id: "q1",
      supplier_id: "s1",
      supplier_name: "Tokyo Bearings",
      currency: "JPY",
      compliance: "compliant",
      reasons: [],
      total_minor: 302000,
      max_lead_time_days: 21,
      rank: 1,
    },
    {
      quotation_id: "q2",
      supplier_id: "s2",
      supplier_name: "Osaka Motion",
      currency: "JPY",
      compliance: "non_compliant",
      reasons: ["insufficient_quantity"],
      total_minor: 1000,
      max_lead_time_days: null,
      rank: null,
    },
  ],
  decisions: [],
  remaining_approvals: 2,
};

/** Route table stand-in for the Rust layer; records every call. */
function fakeBridge(routes: Record<string, unknown>) {
  const calls: { method: string; path: string; body: unknown }[] = [];
  const api = vi.fn(async (method: string, path: string, body?: unknown) => {
    calls.push({ method, path, body });
    if (method !== "GET") return { status: 204, body: null, request_id: "r" };
    const hit = Object.entries(routes).find(([p]) => path === p);
    return hit
      ? { status: 200, body: hit[1], request_id: "r" }
      : { status: 404, body: { error: { code: "not_found" } }, request_id: "r" };
  });
  return { bridge: { api, log: vi.fn() } as unknown as Bridge, calls };
}

const base = `/api/v1/orgs/${ORG}/requests/${REQ}`;

beforeAll(async () => {
  await initI18n("en");
});
afterEach(cleanup);

describe("money input", () => {
  it("parses amounts exactly into minor units without floating point", () => {
    expect(parseMinor("1234", "JPY")).toBe(1234);
    expect(parseMinor("1,234", "JPY")).toBe(1234);
    expect(parseMinor("12.5", "JPY")).toBeNull();
    expect(parseMinor("0.1", "USD")).toBe(10);
    expect(parseMinor("1234.56", "USD")).toBe(123456);
    expect(parseMinor("1234.567", "USD")).toBeNull();
    expect(parseMinor("-5", "USD")).toBeNull();
    expect(parseMinor("1e3", "USD")).toBeNull();
    expect(parseMinor("99999999999999999999", "JPY")).toBeNull();
    expect(minorToInput(123456, "USD")).toBe("1234.56");
    expect(minorToInput(5, "USD")).toBe("0.05");
    expect(minorToInput(500, "JPY")).toBe("500");
  });
});

describe("errors", () => {
  it("shows the specific message for a known detail and the general one otherwise", () => {
    render(<ErrorText code="conflict/items_not_verified" />);
    expect(screen.getByRole("alert").textContent).toBe("Every item must be verified first.");
    cleanup();
    render(<ErrorText code="validation_failed/some_field" />);
    expect(screen.getByRole("alert").textContent).toBe("Please check the information you entered.");
  });
});

describe("request detail", () => {
  it("lets an assigned operator verify an item only against a verified product record", async () => {
    const product = {
      id: "prod-1",
      manufacturer: "NSK",
      mpn: "6204ZZ",
      description: "Ball bearing",
      category: "",
      specs: {},
      status: "verified",
      evidence_count: 1,
      verified_at: "2026-11-01T00:00:00Z",
      created_at: "2026-11-01T00:00:00Z",
    };
    const { bridge, calls } = fakeBridge({
      [base]: detail("identifying"),
      [`${base}/attachments`]: [],
      "/api/v1/products?status=verified&q=6204ZZ": [product],
    });
    render(
      <BridgeProvider value={bridge}>
        <RequestDetail
          active={view("assigned", ["operate_requests", "verify_products", "view_all_requests"])}
          selfId={ME}
          requestId={REQ}
          onBack={() => undefined}
        />
      </BridgeProvider>,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Verify" }));
    const confirm = screen.getByRole("button", { name: "Confirm verified" });
    expect((confirm as HTMLButtonElement).disabled).toBe(true); // a record must be chosen
    // Only verified records are searched for, starting from the item's part number.
    fireEvent.click(await screen.findByRole("radio", { name: /NSK 6204ZZ/ }));
    fireEvent.change(screen.getByLabelText("Why it matches (optional)"), {
      target: { value: "label photo" },
    });
    await act(async () => {
      fireEvent.click(confirm);
    });
    expect(calls).toContainEqual({
      method: "POST",
      path: `${base}/items/item-1/verification`,
      body: { product_id: "prod-1", note: "label photo" },
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Start sourcing" }));
    });
    expect(calls.some((c) => c.method === "POST" && c.path === `${base}/start-sourcing`)).toBe(true);
    // Customer-only actions are not offered to staff.
    expect(screen.queryByRole("button", { name: "Submit to Chuo-Sogo" })).toBeNull();
  });

  it("uploads attachments as base64 and saves downloads through the native layer", async () => {
    const att = {
      id: "att-1",
      item_id: null,
      file_name: "図面.pdf",
      content_type: "application/pdf",
      size_bytes: 2048,
      uploaded_by: "someone-else",
      uploaded_by_name: "Req",
      created_at: "2026-11-01T00:00:00Z",
    };
    const { bridge, calls } = fakeBridge({ [base]: detail("submitted"), [`${base}/attachments`]: [att] });
    const download = vi.fn(async () => ({
      status: 200,
      body: { saved_to: "/home/u/Downloads/図面.pdf" },
      request_id: "r",
    }));
    (bridge as unknown as { download: typeof download }).download = download;
    render(
      <BridgeProvider value={bridge}>
        <RequestDetail
          active={view("assigned", ["operate_requests", "view_all_requests"])}
          selfId={ME}
          requestId={REQ}
          onBack={() => undefined}
        />
      </BridgeProvider>,
    );
    expect(await screen.findByText("図面.pdf")).toBeTruthy();
    expect(screen.getByText("2 KB")).toBeTruthy();
    // Not the uploader: no remove button for this file.
    expect(screen.queryByRole("button", { name: "Remove" })).toBeNull();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save to Downloads" }));
    });
    expect(download).toHaveBeenCalledWith(ORG, "att-1", "図面.pdf");
    expect(screen.getByRole("status").textContent).toContain("/home/u/Downloads/図面.pdf");

    const file = new File(["%PDF-1.7 test"], "spec.pdf", { type: "application/pdf" });
    await act(async () => {
      fireEvent.change(screen.getByLabelText(/^Add a file/), { target: { files: [file] } });
    });
    await waitFor(() =>
      expect(calls.some((c) => c.method === "POST" && c.path === `${base}/attachments`)).toBe(true),
    );
    const upload = calls.find((c) => c.method === "POST" && c.path === `${base}/attachments`);
    expect(upload?.body).toMatchObject({ file_name: "spec.pdf", item_id: null });
    expect(atob((upload?.body as { data_base64: string }).data_base64)).toBe("%PDF-1.7 test");
  });

  it("never offers approval controls to staff, and offers them to customer approvers", async () => {
    const routes = {
      [base]: detail("awaiting_approval", true),
      [`/api/v1/orgs/${ORG}/recommendations/${REC}`]: rec,
    };
    const staff = fakeBridge(routes);
    render(
      <BridgeProvider value={staff.bridge}>
        <RequestDetail
          active={view("assigned", ["operate_requests", "view_all_requests"])}
          selfId={ME}
          requestId={REQ}
          onBack={() => undefined}
        />
      </BridgeProvider>,
    );
    expect(await screen.findByText("Recommendation")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
    cleanup();

    const mgr = fakeBridge(routes);
    render(
      <BridgeProvider value={mgr.bridge}>
        <RequestDetail
          active={view("member", ["approve_recommendation", "review_quotations", "view_all_requests"])}
          selfId={ME}
          requestId={REQ}
          onBack={() => undefined}
        />
      </BridgeProvider>,
    );
    // Deterministic backend result is displayed as-is: the non-compliant quote stays unranked.
    expect(await screen.findByText("Quantity too low")).toBeTruthy();
    expect(screen.getByText("2 required, 2 remaining")).toBeTruthy();
    const reject = screen.getByRole("button", { name: "Reject" }) as HTMLButtonElement;
    expect(reject.disabled).toBe(true); // comment required
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Approve" }));
    });
    expect(mgr.calls).toContainEqual({
      method: "POST",
      path: `/api/v1/orgs/${ORG}/recommendations/${REC}/decisions`,
      body: { decision: "approve", comment: "" },
    });
  });
});

describe("new request", () => {
  it("sends whole-number quantities and refuses invalid ones before calling the server", async () => {
    const created = vi.fn();
    const { bridge, calls } = fakeBridge({});
    (bridge.api as ReturnType<typeof vi.fn>).mockImplementation(
      async (method: string, path: string, body: unknown) => {
        calls.push({ method, path, body });
        return { status: 201, body: { id: "new-id", number: "PR-2026-000002" }, request_id: "r" };
      },
    );
    render(
      <BridgeProvider value={bridge}>
        <NewRequest
          active={view("member", ["submit_request"])}
          onCancel={() => undefined}
          onCreated={created}
        />
      </BridgeProvider>,
    );
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Bearings" } });
    fireEvent.change(screen.getByLabelText("Description", { selector: ".item-fields input" }), {
      target: { value: "Ball bearing" },
    });
    fireEvent.change(screen.getByLabelText("Quantity"), { target: { value: "2.5" } });
    const form = screen.getByRole("button", { name: "Save as draft" }).closest("form");
    if (!form) throw new Error("no form");
    await act(async () => {
      fireEvent.submit(form);
    });
    expect(calls).toHaveLength(0);
    expect(screen.getByRole("alert").textContent).toBe("Enter a whole quantity of at least 1.");

    fireEvent.change(screen.getByLabelText("Quantity"), { target: { value: "40" } });
    await act(async () => {
      fireEvent.submit(form);
    });
    await waitFor(() => expect(created).toHaveBeenCalledWith("new-id"));
    const body = calls[0]?.body as { items: { quantity: number; unit: string }[]; needed_by: unknown };
    expect(body.items[0]).toMatchObject({ quantity: 40, unit: "pcs", description: "Ball bearing" });
    expect(body.needed_by).toBeNull();
  });
});

describe("AI item suggestions", () => {
  it("adds nothing until the requester ticks suggestions, and pre-ticks only clean ones", async () => {
    const { AiItems } = await import("../src/pages/AiItems");
    const suggestion = {
      id: "sug-1",
      capability: "extract_items",
      subject_id: REQ,
      status: "pending",
      provider: "mock",
      model: "rules-v1",
      prompt_version: "extract_items/v1",
      created_at: "2026-11-01T00:00:00Z",
      output: [
        {
          description: "Bearing",
          manufacturer: "NSK",
          mpn: "6204ZZ",
          quantity: 100,
          unit: "pcs",
          confidence: 90,
          flags: [],
          needs_review: false,
        },
        {
          description: "Seal",
          manufacturer: "",
          mpn: "INVENTED",
          quantity: 5,
          unit: "pcs",
          confidence: 30,
          flags: ["mpn_not_in_source"],
          needs_review: true,
        },
      ],
    };
    const calls: { method: string; path: string; body: unknown }[] = [];
    const api = vi.fn(async (method: string, path: string, body?: unknown) => {
      calls.push({ method, path, body });
      if (path.endsWith("/ai/extract-items")) return { status: 201, body: suggestion, request_id: "r" };
      return { status: 200, body: { added: 1 }, request_id: "r" };
    });
    const added = vi.fn();
    render(
      <BridgeProvider value={{ api, log: vi.fn() } as unknown as Bridge}>
        <AiItems active={view("member", ["submit_request"])} requestId={REQ} onAdded={added} />
      </BridgeProvider>,
    );
    fireEvent.change(screen.getByLabelText("Text"), { target: { value: "100 x NSK 6204ZZ" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Suggest items" }));
    });
    const boxes = screen.getAllByRole("checkbox") as HTMLInputElement[];
    expect(boxes.map((b) => b.checked)).toEqual([true, false]);
    expect(screen.getByText("part number not found in the text")).toBeTruthy();
    expect(calls.filter((c) => c.path.includes("accept-items"))).toHaveLength(0);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Add 1 selected" }));
    });
    expect(calls.at(-1)).toEqual({
      method: "POST",
      path: `/api/v1/orgs/${ORG}/ai/suggestions/sug-1/accept-items`,
      body: { indices: [0] },
    });
    expect(added).toHaveBeenCalled();
  });
});

describe("RFQ", () => {
  it("pre-ticks the shortlist with its reasons and sends what staff leave ticked", async () => {
    const pick = (id: string, name: string, selected: boolean, reasons: object[]) => ({
      supplier_id: id,
      name,
      country: "JP",
      score: 10,
      selected,
      relevant: selected,
      reasons,
    });
    const { bridge, calls } = fakeBridge({
      [base]: detail("sourcing"),
      [`${base}/attachments`]: [],
      [`${base}/supplier-shortlist`]: {
        picks: [
          pick("s1", "Tokyo Bearings", true, [{ code: "quoted_before", count: 2 }]),
          pick("s2", "Denki Shoji", true, [{ code: "category", category: "electrical" }]),
          pick("s3", "Kogu Ya", false, []),
        ],
      },
    });
    render(
      <BridgeProvider value={bridge}>
        <RequestDetail
          active={view("assigned", ["operate_requests", "manage_rfqs", "view_all_requests"])}
          selfId={ME}
          requestId={REQ}
          onBack={() => undefined}
        />
      </BridgeProvider>,
    );
    const tokyo = (await screen.findByRole("checkbox", { name: /Tokyo Bearings/ })) as HTMLInputElement;
    expect(tokyo.checked).toBe(true);
    expect((screen.getByRole("checkbox", { name: /Kogu Ya/ }) as HTMLInputElement).checked).toBe(false);
    expect(screen.getByText("Quoted 2 of these items before")).toBeTruthy();
    expect(screen.getByText("Sells electrical")).toBeTruthy();
    expect(screen.getByText("No record with this factory yet")).toBeTruthy();
    fireEvent.click(screen.getByRole("checkbox", { name: /Denki Shoji/ }));
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Create RFQ" }));
    });
    expect(calls).toContainEqual({
      method: "POST",
      path: `${base}/rfqs`,
      body: { supplier_ids: ["s1"], response_due: null, message: "" },
    });
  });

  it("shows when and by whom a draft RFQ was checked, then allows dispatch", async () => {
    const RFQ = "rfq-1";
    const { bridge } = fakeBridge({
      [`/api/v1/orgs/${ORG}/rfqs/${RFQ}`]: {
        rfq: {
          id: RFQ,
          request_id: REQ,
          number: "RFQ-2026-000001",
          status: "draft",
          message: "",
          response_due: null,
          created_at: "2026-11-01T00:00:00Z",
          sent_at: null,
          checked_at: "2026-11-02T03:04:05.123456Z",
          checked_by_name: "Operator",
          check_note: "",
        },
        recipients: [{ supplier_id: "s1", supplier_name: "Tokyo Bearings", status: "pending" }],
        quotations: [],
        latest_comparison_id: null,
        documents: [],
      },
    });
    render(
      <BridgeProvider value={bridge}>
        <RfqPanel
          active={view("assigned", ["manage_rfqs"])}
          rfqId={RFQ}
          requestId={REQ}
          requestStatus="sourcing"
          currency="JPY"
          items={[]}
          onChanged={() => undefined}
        />
      </BridgeProvider>,
    );
    expect(await screen.findByText(/Checked by Operator on/)).toBeTruthy();
    expect((screen.getByRole("button", { name: "Dispatch (test mode)" }) as HTMLButtonElement).disabled).toBe(
      false,
    );
  });
});
