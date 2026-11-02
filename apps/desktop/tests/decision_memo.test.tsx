import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import i18n from "i18next";
import { initI18n } from "../src/i18n";
import type { ComparisonResult } from "../src/lib/types";
import { DecisionMemoPanel } from "../src/pages/RfqPanel";

function result(id: string, name: string, memo?: ComparisonResult["memo"]): ComparisonResult {
  const r: ComparisonResult = {
    quotation_id: id,
    supplier_id: `s-${id}`,
    supplier_name: name,
    currency: "JPY",
    compliance: "compliant",
    reasons: [],
    total_minor: 100000,
    max_lead_time_days: 10,
    rank: 1,
    price_checks: [],
    price_alerts: 0,
    price_alert_bps: null,
  };
  if (memo) r.memo = memo;
  return r;
}

beforeAll(async () => {
  await initI18n("en");
});
afterEach(cleanup);

describe("decision memo", () => {
  it("names the recommendation, its reasons and what is not recommended", async () => {
    await i18n.changeLanguage("en");
    render(
      <DecisionMemoPanel
        results={[
          result("b", "Osaka Parts", {
            score: 714,
            rank: 2,
            factors: [{ factor: "price", weight: 10, points: 60 }],
            cautions: ["over_max_lead_time"],
            preferred_supplier: false,
            past_purchases: 0,
          }),
          result("a", "Tokyo Bearings", {
            score: 771,
            rank: 1,
            factors: [
              { factor: "price", weight: 10, points: 100 },
              { factor: "lead_time", weight: 4, points: 20 },
            ],
            cautions: [],
            preferred_supplier: true,
            past_purchases: 3,
          }),
          result("c", "Cheap Co", {
            score: null,
            rank: null,
            factors: [],
            cautions: ["excluded_supplier"],
            preferred_supplier: false,
            past_purchases: 0,
          }),
        ]}
      />,
    );
    expect(screen.getByText("Recommended: Tokyo Bearings")).toBeTruthy();
    expect(screen.getByText("771 / 1000")).toBeTruthy();
    expect(screen.getByText("Price 100×10 · Lead time 20×4")).toBeTruthy();
    expect(screen.getByText("3 past purchases")).toBeTruthy();
    expect(screen.getByText("beyond the maximum lead time")).toBeTruthy();
    expect(screen.getByText(/Cheap Co not recommended/).textContent).toContain("excluded supplier");
    // Rank order follows the memo, not the input order.
    const rows = screen.getAllByRole("row").slice(1);
    expect(rows[0]?.textContent).toContain("Tokyo Bearings");
  });

  it("shows nothing for runs stored before the memo existed", () => {
    const { container } = render(<DecisionMemoPanel results={[result("a", "X")]} />);
    expect(container.textContent).toBe("");
  });
});
