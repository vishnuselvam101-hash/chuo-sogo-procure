import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { initI18n } from "../src/i18n";
import { BridgeProvider } from "../src/lib/BridgeContext";
import type { Bridge } from "../src/lib/bridge";
import type { MembershipView } from "../src/lib/types";
import { Records } from "../src/pages/Records";

const ORG = "0192f0aa-1111-7222-8333-444455556666";
const active: MembershipView = {
  org_id: ORG,
  org_name: "株式会社エー製作所",
  org_kind: "customer",
  default_locale: "en",
  timezone: "Asia/Tokyo",
  role: "finance",
  permissions: ["view_finance"],
  access: "member",
};

const rows = {
  truncated: false,
  rows: [
    {
      kind: "quotation",
      id: "q1",
      date: "2026-10-02",
      amount_minor: 302000,
      currency: "JPY",
      counterparty: "Tokyo Bearings KK",
      reference: "RFQ-2026-000001",
      request_number: "PR-2026-000001",
      document_id: "d1",
      document_name: "reply.eml",
      document_sha256: "ab".repeat(32),
    },
    {
      kind: "order",
      id: "o1",
      date: "2026-10-02",
      amount_minor: 302000,
      currency: "JPY",
      counterparty: "Tokyo Bearings KK",
      reference: "PO-R-1",
      request_number: "PR-2026-000001",
      document_id: null,
      document_name: null,
      document_sha256: null,
    },
  ],
};

beforeAll(async () => {
  await initI18n("en");
});
afterEach(cleanup);

describe("records register", () => {
  it("searches by date, amount and counterparty and saves the file as received", async () => {
    const paths: string[] = [];
    const api = vi.fn(async (_m: string, path: string) => {
      paths.push(path);
      return { status: 200, body: rows, request_id: "r" };
    });
    const documentDownload = vi.fn(async () => ({
      status: 200,
      body: { saved_to: "/home/u/Downloads/reply.eml" },
      request_id: "r",
    }));
    const bridge = { api, log: vi.fn(), documentDownload } as unknown as Bridge;
    render(
      <BridgeProvider value={bridge}>
        <Records active={active} />
      </BridgeProvider>,
    );
    expect(await screen.findByText("PO-R-1")).toBeTruthy();
    expect(screen.getByTestId("records").textContent).toContain("Quotation");
    expect(screen.getByText(`SHA-256 ${"ab".repeat(6)}…`)).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Counterparty"), { target: { value: "Tokyo" } });
    fireEvent.change(screen.getByLabelText("Amount from (¥)"), { target: { value: "300,000" } });
    fireEvent.change(screen.getByLabelText("From"), { target: { value: "2026-10-01" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Search" }));
    });
    expect(paths.at(-1)).toBe(
      `/api/v1/orgs/${ORG}/records?from=2026-10-01&counterparty=Tokyo&min_minor=300000`,
    );
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "reply.eml" }));
    });
    expect(documentDownload).toHaveBeenCalledWith(ORG, "d1", "reply.eml");
    expect(screen.getByText("Saved to /home/u/Downloads/reply.eml")).toBeTruthy();
  });
});
