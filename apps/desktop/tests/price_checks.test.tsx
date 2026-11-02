import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import i18n from "i18next";
import { initI18n } from "../src/i18n";
import type { ComparisonResult, ItemView } from "../src/lib/types";
import { ComparisonTable } from "../src/pages/RfqPanel";

const base = {
  currency: "JPY",
  compliance: "compliant",
  reasons: [],
  max_lead_time_days: 14,
};

const results: ComparisonResult[] = [
  {
    ...base,
    quotation_id: "q1",
    supplier_id: "s1",
    supplier_name: "Tokyo Bearings",
    total_minor: 300000,
    rank: 1,
    price_alert_bps: 1000,
    price_alerts: 1,
    price_checks: [
      {
        request_item_id: "i1",
        quoted_unit_price_minor: 1200,
        last_unit_price_milli: 1_100_000,
        last_currency: "JPY",
        last_purchased_on: "2026-03-01",
        last_supplier_name: "Suzuki Kizai",
        change_bps: 909,
        above_alert: false,
      },
      {
        request_item_id: "i2",
        quoted_unit_price_minor: 45000,
        last_unit_price_milli: 40_000_000,
        last_currency: "JPY",
        last_purchased_on: "2026-02-01",
        last_supplier_name: "",
        change_bps: 1250,
        above_alert: true,
      },
    ],
  },
  {
    ...base,
    currency: "USD",
    quotation_id: "q2",
    supplier_id: "s2",
    supplier_name: "Pacific Parts",
    total_minor: null,
    rank: null,
    price_alert_bps: 1000,
    price_alerts: 0,
    price_checks: [
      {
        request_item_id: "i1",
        quoted_unit_price_minor: 800,
        last_unit_price_milli: 1_100_000,
        last_currency: "JPY",
        last_purchased_on: "2026-03-01",
        last_supplier_name: "Suzuki Kizai",
        change_bps: null,
        above_alert: false,
      },
    ],
  },
];

const items = [
  { id: "i1", line_no: 1, description: "Ball bearing" },
  { id: "i2", line_no: 2, description: "Linear guide" },
] as ItemView[];

beforeAll(async () => {
  await initI18n("en");
});

afterEach(async () => {
  cleanup();
  await i18n.changeLanguage("en");
});

describe("comparison price checks", () => {
  it("shows the last price, the change and the alert count per supplier", () => {
    render(<ComparisonTable results={results} currency="JPY" items={items} />);
    expect(screen.getByText("Price alerts")).toBeTruthy();
    expect(screen.getByText("Line +10%")).toBeTruthy();

    const bearing = screen.getByText(
      /1\. Ball bearing: quoted JP¥1,200 \/ last JP¥1,100 \(Suzuki Kizai, 1 Mar 2026\)/,
    );
    expect(bearing.textContent).toContain("vs last +9.09%");
    expect(bearing.className).not.toContain("error");

    const guide = screen.getByText(/2\. Linear guide: quoted JP¥45,000 \/ last JP¥40,000 \(Unknown supplier/);
    expect(guide.textContent).toContain("vs last +12.5%");
    expect(guide.textContent).toContain("Above alert line");
    expect(guide.className).toContain("error");

    // Different currency: shown, never converted, never flagged.
    const usd = screen.getByText(/quoted US\$8\.00 \/ last JP¥1,100/);
    expect(usd.textContent).toContain("different currency");
    expect(usd.className).not.toContain("error");

    const rows = screen.getAllByRole("row");
    const tokyo = rows.find((r) => r.textContent?.includes("Tokyo Bearings"));
    const alertCell = tokyo?.querySelector("td.error");
    expect(alertCell?.textContent).toBe("1");
  });

  it("renders in Japanese with a signed decrease", async () => {
    await i18n.changeLanguage("ja");
    const first = results[0];
    if (!first?.price_checks?.[0]) throw new Error("fixture");
    const cheaper: ComparisonResult = {
      ...first,
      price_alerts: 0,
      price_checks: [{ ...first.price_checks[0], quoted_unit_price_minor: 1045, change_bps: -500 }],
    };
    render(<ComparisonTable results={[cheaper]} currency="JPY" items={items} />);
    expect(screen.getByText("前回比アラート")).toBeTruthy();
    expect(screen.getByText("前回比 -5%")).toBeTruthy();
    expect(screen.getByText("0件")).toBeTruthy();
  });

  it("adds nothing when the server sent no price checks", () => {
    // A run stored before price checks existed carries none of the fields.
    const plain: ComparisonResult[] = results.map((r) => {
      const copy: Partial<ComparisonResult> = { ...r };
      delete copy.price_checks;
      delete copy.price_alerts;
      delete copy.price_alert_bps;
      return copy as ComparisonResult;
    });
    render(<ComparisonTable results={plain} currency="JPY" />);
    expect(screen.queryByText("Price alerts")).toBeNull();
    expect(screen.queryByText(/vs last/)).toBeNull();
    expect(screen.getAllByRole("row")).toHaveLength(3);
  });
});
