import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { initI18n } from "../src/i18n";
import { ConfirmButton } from "../src/components/ui";
import { RequestGuide } from "../src/components/RequestGuide";
import { applyDisplay } from "../src/lib/display";

beforeAll(async () => {
  await initI18n("en");
});
afterEach(cleanup);

describe("reading comfort and guidance", () => {
  it("puts the text size and contrast on <html>, ignoring unknown sizes", () => {
    applyDisplay({ text_size: "xlarge", high_contrast: true });
    expect(document.documentElement.dataset.textSize).toBe("xlarge");
    expect(document.documentElement.dataset.contrast).toBe("high");
    applyDisplay({ text_size: "huge" as never, high_contrast: false });
    expect(document.documentElement.dataset.textSize).toBe("normal");
    expect(document.documentElement.dataset.contrast).toBeUndefined();
  });

  it("asks before a destructive action and can be backed out of", () => {
    const onConfirm = vi.fn();
    render(<ConfirmButton label="Remove" onConfirm={onConfirm} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect(onConfirm).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    fireEvent.click(screen.getByRole("button", { name: "Yes, Remove" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("explains where a request is and what happens next", () => {
    render(<RequestGuide status="awaiting_approval" />);
    expect(screen.getByText("Step 4 of 6")).toBeTruthy();
    expect(screen.getByText("This request is waiting for approval in your company.")).toBeTruthy();
    expect(document.querySelector('[aria-current="step"]')?.textContent).toContain("Approval");
  });

  it("has a now/next text for every request status", async () => {
    const i18n = (await import("../src/i18n")).default;
    const statuses = [
      "draft",
      "submitted",
      "needs_clarification",
      "identifying",
      "sourcing",
      "rfq_sent",
      "quotes_received",
      "under_comparison",
      "recommended",
      "awaiting_approval",
      "approved",
      "rejected",
      "ordered",
      "delivered",
      "invoiced",
      "disputed",
      "reconciled",
      "closed",
      "cancelled",
    ];
    for (const s of statuses) {
      expect(i18n.exists(`procurement:guide.status.${s}.now`)).toBe(true);
      expect(i18n.exists(`procurement:guide.status.${s}.next`)).toBe(true);
    }
  });
});
