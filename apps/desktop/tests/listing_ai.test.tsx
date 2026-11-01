import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { AiAssist, applySuggestion, applyTranslation, type ListingAiOutput } from "../src/catalogue/AiAssist";
import { initI18n } from "../src/i18n";
import { BridgeProvider } from "../src/lib/BridgeContext";
import type { Bridge } from "../src/lib/bridge";

const suggestion: ListingAiOutput = {
  manufacturer: "NSK",
  model_number: "6204-2RS",
  condition: "new",
  title: "深溝玉軸受",
  title_en: "",
  description: "",
  description_en: "",
  specs: [
    { key: "内径", value: "20 mm", in_source: true },
    { key: "外径", value: "52 mm", in_source: false },
  ],
  confidence: 40,
  flags: ["spec_not_in_source"],
  needs_review: true,
};

const empty = {
  manufacturer: "",
  model_number: "",
  condition: "new",
  title: "",
  title_en: "Bearing",
  description: "",
  description_en: "",
  specs: [{ key: "内径", value: "?" }],
};

beforeAll(async () => {
  await initI18n("en");
});
afterEach(cleanup);

describe("listing assistant", () => {
  it("fills empty fields only and never overwrites what is already there", () => {
    const f = applySuggestion(empty, suggestion);
    expect(f.model_number).toBe("6204-2RS");
    expect(f.title).toBe("深溝玉軸受");
    expect(f.title_en).toBe("Bearing");
    expect(f.specs).toEqual([
      { key: "内径", value: "?" }, // the supplier's own value stays
      { key: "外径", value: "52 mm" },
    ]);
    // A second suggestion (e.g. from a catalogue file) cannot replace the first one's name.
    const again = applySuggestion(f, { ...suggestion, title: "外径: 47 mm", model_number: "X" });
    expect(again.title).toBe("深溝玉軸受");
    expect(again.model_number).toBe("6204-2RS");
    expect(
      applySuggestion({ ...empty, condition: "new" }, { ...suggestion, condition: "used" }).condition,
    ).toBe("used");
    expect(applySuggestion({ ...empty, condition: "refurbished" }, suggestion).condition).toBe("refurbished");
  });

  it("a translation draft never replaces text the supplier wrote", () => {
    const d = { target: "en" as const, title: "Bearing 6204", description: "Bore 20 mm", flags: [] };
    expect(applyTranslation({ ...empty, title_en: "My title", description_en: "" }, d)).toMatchObject({
      title_en: "My title",
      description_en: "Bore 20 mm",
    });
  });

  it("shows what could not be confirmed and records the decision before applying", async () => {
    const calls: string[] = [];
    const api = vi.fn(async (method: string, path: string) => {
      calls.push(`${method} ${path}`);
      if (path.endsWith("/from-text"))
        return {
          status: 201,
          body: {
            id: "s1",
            status: "pending",
            output: suggestion,
            error_code: null,
            input_text: "x",
            input_source: "typed",
          },
          request_id: "r",
        };
      return { status: 200, body: {}, request_id: "r" };
    });
    const onApply = vi.fn();
    render(
      <BridgeProvider value={{ api } as unknown as Bridge}>
        <AiAssist orgBase="/api/v1/orgs/o/listings" listingId={null} onApply={onApply} />
      </BridgeProvider>,
    );
    fireEvent.change(screen.getByRole("textbox", { name: "Notes about the product" }), {
      target: { value: "型番: 6204-2RS" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Suggest fields" }));
    expect(await screen.findByText("Some specification values are not in your description.")).toBeTruthy();
    expect(screen.getByText(/52 mm — not in your description/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Apply to the form" }));
    await vi.waitFor(() => expect(onApply).toHaveBeenCalledWith(suggestion));
    expect(calls).toEqual([
      "POST /api/v1/orgs/o/listings/ai/from-text",
      "POST /api/v1/orgs/o/listings/ai/suggestions/s1/decision",
    ]);
  });
});
