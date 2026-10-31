import { describe, expect, it } from "vitest";
import { formatDateTime, formatMoney } from "../src/lib/format";

describe("display formatting", () => {
  it("formats yen without decimals in every language", () => {
    for (const lang of ["ja", "en", "ta", "hi"]) {
      const s = formatMoney(1_280_000, "JPY", lang);
      expect(s, lang).toMatch(/12,80,000|1,280,000/);
      expect(s, lang).not.toMatch(/\.\d/);
    }
    expect(formatMoney(1_280_000, "JPY", "ja")).toContain("1,280,000");
  });

  it("uses minor units for two-decimal currencies", () => {
    expect(formatMoney(123_456, "USD", "en")).toContain("1,234.56");
  });

  it("shows instants in the organisation's time zone", () => {
    // 01:30 UTC is 10:30 in Tokyo.
    expect(formatDateTime("2026-04-01T01:30:00Z", "ja", "Asia/Tokyo")).toContain("10:30");
    expect(formatDateTime("2026-04-01T01:30:00Z", "ja", "Asia/Tokyo")).toContain("2026");
  });
});
