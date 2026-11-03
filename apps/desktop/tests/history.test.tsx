import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { initI18n } from "../src/i18n";
import { BridgeProvider } from "../src/lib/BridgeContext";
import type { Bridge } from "../src/lib/bridge";
import { bpsToPercent, formatMilliPrice, formatMilliQuantity, percentToBps } from "../src/lib/format";
import { navFor } from "../src/lib/nav";
import type { MembershipView } from "../src/lib/types";
import { toCriteriaInput } from "../src/pages/BuyingCriteria";
import { PriceReference } from "../src/pages/PriceReference";
import { PurchaseHistory } from "../src/pages/PurchaseHistory";

const ORG = "o1";

function member(permissions: string[], access = "member"): MembershipView {
  return {
    org_id: ORG,
    org_name: "A",
    org_kind: "customer",
    default_locale: "ja",
    timezone: "Asia/Tokyo",
    role: "",
    permissions,
    access,
  } as MembershipView;
}

const EMPTY_CRITERIA = {
  priorities: [],
  preferred_suppliers: [],
  excluded_suppliers: [],
  required_certifications: [],
  max_lead_time_days: null,
  price_alert_bps: null,
  notes: "",
  updated_by_name: null,
  updated_at: null,
};

const OPPORTUNITIES = {
  months: 12,
  share_bps: 5000,
  totals: [{ currency: "JPY", items: 2, spend_minor: 375000, potential_minor: 25000, fee_minor: 12500 }],
  items: [
    {
      item_key: "m:VVF20",
      description: "VVF cable 100m",
      currency: "JPY",
      purchases: 3,
      suppliers: 2,
      spend_minor: 375000,
      min_price_milli: 10000000,
      max_price_milli: 12000000,
      potential_minor: 25000,
      fee_minor: 12500,
      single_supplier: false,
    },
    {
      item_key: "m:EM6",
      description: "End mill 6mm",
      currency: "JPY",
      purchases: 2,
      suppliers: 1,
      spend_minor: 24000,
      min_price_milli: 3000000,
      max_price_milli: 3000000,
      potential_minor: 0,
      fee_minor: 0,
      single_supplier: true,
    },
  ],
};

function fakeBridge(get: Record<string, unknown>, post: Record<string, unknown> = {}) {
  const calls: { method: string; path: string; body: unknown }[] = [];
  const api = vi.fn(async (method: string, path: string, body?: unknown) => {
    calls.push({ method, path, body });
    const table = method === "GET" ? get : post;
    const hit = Object.entries(table).find(([p]) => path.startsWith(p));
    return hit
      ? { status: method === "POST" && path.endsWith("/imports") ? 201 : 200, body: hit[1], request_id: "r" }
      : { status: 404, body: { error: { code: "not_found" } }, request_id: "r" };
  });
  return { bridge: { api, log: vi.fn() } as unknown as Bridge, calls };
}

beforeAll(async () => {
  await initI18n("en");
});
afterEach(cleanup);

describe("purchase history", () => {
  it("is in the menu for buyers and finance, not for requesters", () => {
    const has = (p: string[]) => navFor(member(p)).some((i) => i.id === "history");
    expect(has(["manage_purchase_history"])).toBe(true);
    expect(has(["view_all_requests"])).toBe(true);
    expect(has(["submit_request", "view_own_requests"])).toBe(false);
  });

  it("shows exact sub-yen prices and fractional quantities", () => {
    expect(formatMilliPrice(850, "JPY", "en")).toBe("JP¥0.85");
    expect(formatMilliPrice(320_000, "JPY", "en")).toBe("JP¥320");
    expect(formatMilliPrice(123_450, "USD", "en")).toBe("US$1.2345");
    expect(formatMilliQuantity(2500, "en")).toBe("2.5");
  });

  it("reads criteria percentages exactly", () => {
    expect(percentToBps("10")).toBe(1000);
    expect(percentToBps("12.5")).toBe(1250);
    expect(percentToBps("0.05")).toBe(5);
    expect(percentToBps("1.234")).toBeNull();
    expect(percentToBps("-1")).toBeNull();
    expect(bpsToPercent(1250)).toBe("12.5");
    expect(bpsToPercent(1000)).toBe("10");
    const form = {
      priorities: ["lead_time", "", "price"],
      preferred: "山田商事\n\n 鈴木機材 ",
      excluded: "",
      certifications: "ISO 9001",
      maxLead: "14",
      alert: "10",
      notes: "",
    };
    expect(toCriteriaInput(form)).toEqual({
      priorities: ["lead_time", "price"],
      preferred_suppliers: ["山田商事", "鈴木機材"],
      excluded_suppliers: [],
      required_certifications: ["ISO 9001"],
      max_lead_time_days: 14,
      price_alert_bps: 1000,
      notes: "",
    });
    expect(toCriteriaInput({ ...form, maxLead: "two weeks" })).toBeNull();
    expect(toCriteriaInput({ ...form, alert: "ten" })).toBeNull();
  });

  it("previews a file, lets the columns be corrected, then imports", async () => {
    const preview = {
      header_row: 0,
      headers: ["日付", "品名", "単価", "担当"],
      mapping: ["date", "description", "unit_price", null],
      data_rows: 3,
      valid_rows: 2,
      error_counts: { date_invalid: 1 },
      errors: [{ row: 4, code: "date_invalid" }],
      sample: [
        {
          row: 2,
          purchased_on: "2026-01-05",
          supplier_name: "",
          description: "六角ボルト",
          manufacturer: "",
          mpn: "",
          quantity_milli: 1000,
          unit: "",
          unit_price_milli: 12_500,
          price_derived: false,
          currency: "JPY",
          lead_time_days: null,
        },
      ],
    };
    const base = `/api/v1/orgs/${ORG}`;
    const { bridge, calls } = fakeBridge(
      {
        [`${base}/purchase-history/imports`]: [],
        [`${base}/purchase-history/opportunities`]: OPPORTUNITIES,
        [`${base}/purchase-history`]: [],
        [`${base}/buying-criteria`]: EMPTY_CRITERIA,
      },
      {
        [`${base}/purchase-history/preview`]: preview,
        [`${base}/purchase-history/imports`]: {
          id: "i1",
          file_name: "h.csv",
          rows_imported: 2,
          rows_skipped: 1,
          imported_by_name: "Admin",
          imported_at: "2026-11-01T00:00:00Z",
          withdrawn_at: null,
        },
      },
    );
    render(
      <BridgeProvider value={bridge}>
        <PurchaseHistory active={member(["manage_purchase_history"])} />
      </BridgeProvider>,
    );
    const input = await screen.findByLabelText("File");
    fireEvent.change(input, {
      target: { files: [new File(["日付,品名,単価"], "h.csv", { type: "text/csv" })] },
    });
    expect(await screen.findByText("2 of 3 rows can be imported.")).toBeTruthy();
    expect(screen.getByText(/Date not readable/)).toBeTruthy();
    expect(screen.getByText("JP¥12.5")).toBeTruthy();

    // Changing a column asks for a fresh check before anything can be imported.
    fireEvent.change(screen.getByLabelText("Contents of column 担当"), { target: { value: "supplier" } });
    expect(screen.queryByRole("button", { name: /Import 2 rows/ })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Check again with these columns" }));
    await waitFor(() => expect(calls.filter((c) => c.path.endsWith("/preview")).length).toBe(2));
    const recheck = calls.filter((c) => c.path.endsWith("/preview"))[1]?.body as Record<string, unknown>;
    expect(recheck.mapping).toEqual(["date", "description", "unit_price", "supplier"]);
    expect(recheck.header_row).toBe(0);

    fireEvent.click(await screen.findByRole("button", { name: "Import 2 rows" }));
    expect(await screen.findByText("Imported 2 rows (1 skipped).")).toBeTruthy();
  });

  it("hides import controls from people who may only read", async () => {
    const base = `/api/v1/orgs/${ORG}`;
    const { bridge } = fakeBridge({
      [`${base}/purchase-history/imports`]: [],
      [`${base}/purchase-history/opportunities`]: OPPORTUNITIES,
      [`${base}/purchase-history`]: [],
      [`${base}/buying-criteria`]: EMPTY_CRITERIA,
    });
    render(
      <BridgeProvider value={bridge}>
        <PurchaseHistory active={member(["view_all_requests"])} />
      </BridgeProvider>,
    );
    expect(await screen.findByText("Nothing imported yet.")).toBeTruthy();
    // Where the factory can save, from its own records; one-supplier items flagged.
    const opp = screen.getByTestId("opportunities");
    expect(opp.textContent).toContain("VVF cable 100m");
    expect(opp.textContent).toContain("Always the same supplier: never compared");
    expect(opp.textContent).toContain("Our share at 50%");
    expect(screen.queryByLabelText("File")).toBeNull();
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
  });

  it("shows what was paid before next to request lines", async () => {
    const base = `/api/v1/orgs/${ORG}`;
    const point = (date: string, supplier: string, milli: number) => ({
      purchased_on: date,
      supplier_name: supplier,
      unit_price_milli: milli,
      currency: "JPY",
    });
    const { bridge } = fakeBridge({
      [`${base}/requests/r1/price-reference`]: [
        {
          item_id: "a",
          line_no: 1,
          matched_on: "mpn",
          summary: {
            purchases: 2,
            last: point("2025-09-01", "鈴木機材", 310_000),
            min_price_milli: 310_000,
            max_price_milli: 320_000,
            cheapest: point("2025-09-01", "鈴木機材", 310_000),
            suppliers: 2,
          },
          recent: [],
        },
        { item_id: "b", line_no: 2, matched_on: "mpn", summary: null, recent: [] },
      ],
      [`${base}/buying-criteria`]: {
        ...EMPTY_CRITERIA,
        priorities: ["lead_time", "price"],
        max_lead_time_days: 14,
      },
    });
    render(
      <BridgeProvider value={bridge}>
        <PriceReference active={member(["view_all_requests"])} requestId="r1" />
      </BridgeProvider>,
    );
    expect(await screen.findByText("JP¥310")).toBeTruthy();
    expect(screen.getByText("JP¥310 – JP¥320")).toBeTruthy();
    expect(screen.getByText("Not bought before")).toBeTruthy();
    expect(await screen.findByText("Lead time › Price")).toBeTruthy();
  });
});

describe("item aliases", () => {
  it("proposes similar past items for an unmatched line and links one", async () => {
    const base = `/api/v1/orgs/${ORG}`;
    const { bridge, calls } = fakeBridge(
      {
        [`${base}/requests/r1/price-reference`]: [
          { item_id: "a", line_no: 1, item_key: "m:62042Z", matched_on: "mpn", summary: null, recent: [] },
        ],
        [`${base}/requests/r1/items/a/similar-items`]: [
          {
            item_key: "m:6204ZZ",
            description: "深溝玉軸受",
            mpn: "6204ZZ",
            purchases: 2,
            last_purchased_on: "2025-06-01",
            last_unit_price_milli: 330_000,
            currency: "JPY",
            score: 666,
          },
        ],
        [`${base}/buying-criteria`]: EMPTY_CRITERIA,
      },
      { [`${base}/item-aliases`]: { id: "x" } },
    );
    render(
      <BridgeProvider value={bridge}>
        <PriceReference active={member(["manage_purchase_history"])} requestId="r1" />
      </BridgeProvider>,
    );
    fireEvent.click(await screen.findByText("Find similar items"));
    expect(await screen.findByText("深溝玉軸受")).toBeTruthy();
    fireEvent.click(screen.getByText("Same item"));
    await waitFor(() =>
      expect(calls.find((c) => c.method === "POST")?.body).toEqual({
        alias_key: "m:62042Z",
        canonical_key: "m:6204ZZ",
      }),
    );
  });

  it("lets readers see proposals but not link them", async () => {
    const base = `/api/v1/orgs/${ORG}`;
    const { bridge } = fakeBridge({
      [`${base}/requests/r1/price-reference`]: [
        {
          item_id: "a",
          line_no: 1,
          item_key: "d:ナット",
          matched_on: "description",
          summary: null,
          recent: [],
        },
      ],
      [`${base}/requests/r1/items/a/similar-items`]: [],
      [`${base}/buying-criteria`]: EMPTY_CRITERIA,
    });
    render(
      <BridgeProvider value={bridge}>
        <PriceReference active={member(["view_all_requests"])} requestId="r1" />
      </BridgeProvider>,
    );
    fireEvent.click(await screen.findByText("Find similar items"));
    expect(await screen.findByText("No similar past items were found.")).toBeTruthy();
    expect(screen.queryByText("Same item")).toBeNull();
  });
});
