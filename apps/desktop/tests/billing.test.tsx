import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { toLines } from "../src/admin/InvoiceEditor";
import { InvoiceView, PdfButton, rateLabel } from "../src/billing/InvoiceView";
import { initI18n } from "../src/i18n";
import { BridgeProvider } from "../src/lib/BridgeContext";
import type { Bridge } from "../src/lib/bridge";
import type { IssuedInvoice } from "../src/lib/types";

const row = (quantity: string, price: string) => ({
  description: "x",
  quantity,
  unit: "",
  price,
  rate: 1000,
});

function doc(over: Partial<IssuedInvoice> = {}): IssuedInvoice {
  return {
    id: "i1",
    org_id: "o1",
    org_name: "株式会社エー",
    kind: "invoice",
    status: "issued",
    number: "CS-2026-00001",
    original_id: null,
    issue_date: "2026-11-01",
    due_date: "2026-12-01",
    currency: "JPY",
    total_minor: 1324,
    overdue: false,
    created_at: "2026-11-01T00:00:00Z",
    recipient_name: "株式会社エー",
    recipient_address: "",
    request_id: null,
    notes: "",
    reason: "",
    tax_rounding: "floor",
    subtotal_minor: 1209,
    tax_minor: 115,
    per_rate: [
      { rate_bps: 1000, base_minor: 999, tax_minor: 99 },
      { rate_bps: 800, base_minor: 210, tax_minor: 16 },
    ],
    lines: [
      {
        position: 1,
        description: "サービス料",
        quantity: 3,
        unit: "式",
        unit_price_minor: 333,
        tax_rate_bps: 1000,
      },
      { position: 2, description: "飲料", quantity: 2, unit: "", unit_price_minor: 105, tax_rate_bps: 800 },
    ],
    credit_notes: [],
    paid_on: null,
    paid_note: "",
    pdf_sha256: "ab".repeat(32),
    ...over,
  };
}

beforeAll(async () => {
  await initI18n("en");
});
afterEach(cleanup);

describe("billing", () => {
  it("reads typed lines exactly (no floats) and refuses unreadable rows", () => {
    expect(toLines([row("3", "333")], "JPY")).toEqual({
      lines: [{ description: "x", quantity: 3, unit: "", unit_price_minor: 333, tax_rate_bps: 1000 }],
    });
    expect(toLines([row("1", "12.34")], "USD")).toMatchObject({ lines: [{ unit_price_minor: 1234 }] });
    expect(toLines([row("1", "1"), row("0", "1")], "JPY")).toEqual({ bad: 1 });
    expect(toLines([row("1.5", "1")], "JPY")).toEqual({ bad: 0 });
    expect(toLines([row("1", "12.5")], "JPY")).toEqual({ bad: 0 });
    expect(toLines([row("1", "-5")], "JPY")).toEqual({ bad: 0 });
  });

  it("labels the reduced rate", () => {
    expect(rateLabel(1000)).toBe("10%");
    expect(rateLabel(800)).toBe("8%※");
    expect(rateLabel(0)).toBe("0%");
  });

  it("shows the server's per-rate totals, not its own arithmetic", () => {
    render(<InvoiceView doc={doc()} />);
    expect(screen.getByTestId("per-rate").textContent).toContain("Subject to 8%※");
    expect(screen.getByTestId("invoice-total").textContent).toContain("1,324");
    expect(screen.getByText("※ reduced tax rate (8 %) item")).toBeTruthy();
  });

  it("downloads the PDF through the native layer by number", async () => {
    const invoicePdf = vi.fn(async () => ({
      status: 200,
      body: { saved_to: "/home/u/Downloads/CS-2026-00001.pdf" },
      request_id: "r",
    }));
    render(
      <BridgeProvider value={{ invoicePdf } as unknown as Bridge}>
        <PdfButton doc={doc()} />
      </BridgeProvider>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Download PDF" }));
    expect(await screen.findByRole("status")).toHaveProperty(
      "textContent",
      "Saved to /home/u/Downloads/CS-2026-00001.pdf",
    );
    expect(invoicePdf).toHaveBeenCalledWith("o1", "i1", "CS-2026-00001");
  });

  it("drafts have no PDF button", () => {
    render(<PdfButton doc={doc({ status: "draft", number: null })} />);
    expect(screen.queryByRole("button")).toBeNull();
  });
});
