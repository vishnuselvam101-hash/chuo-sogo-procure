import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { addToCart, expectedTotal, loadCart, saveCart } from "../src/shop/cart";
import { Legal } from "../src/shop/ShopPages";
import { initI18n } from "../src/i18n";
import type { ShopSettings } from "../src/lib/types";

const line = (id: string, price: number, qty: number) => ({
  listing_id: id,
  title: id,
  model_number: "M",
  unit_price_minor: price,
  quantity: qty,
});

beforeAll(async () => {
  await initI18n("en");
});
afterEach(cleanup);

describe("shop", () => {
  it("expects the same totals the server computes (shipping free from the limit)", () => {
    expect(expectedTotal([line("a", 1100, 2)], 880, 10000)).toEqual({
      items: 2200,
      shipping: 880,
      total: 3080,
    });
    expect(expectedTotal([line("a", 1100, 10)], 880, 10000)).toEqual({
      items: 11000,
      shipping: 0,
      total: 11000,
    });
    expect(expectedTotal([line("a", 1100, 10)], 880, null).shipping).toBe(880);
  });

  it("merges the same product, caps quantities, survives bad storage", () => {
    const merged = addToCart([line("a", 1000, 999)], line("a", 1100, 5));
    expect(merged).toEqual([{ ...line("a", 1100, 1000) }]);
    saveCart("o", merged);
    expect(loadCart("o")).toEqual(merged);
    localStorage.setItem("procure.shop.cart.x", "{not json");
    expect(loadCart("x")).toEqual([]);
    localStorage.setItem("procure.shop.cart.y", JSON.stringify([{ listing_id: "a", quantity: -3 }, 7]));
    expect(loadCart("y")).toEqual([]);
  });

  it("shows the seller notice required for consumer sales", () => {
    const s = {
      open: true,
      shipping_fee_minor: 880,
      free_shipping_over_minor: null,
      payment_days: 7,
      seller_name: "株式会社中央総合",
      responsible_person: "山田太郎",
      seller_address: "大阪府",
      seller_phone: "06-0000-0000",
      seller_email: "shop@example.test",
      price_info: "税込",
      payment_info: "銀行振込",
      delivery_info: "3営業日",
      returns_policy: "8日以内",
      privacy_policy: "配送のみ",
    } satisfies ShopSettings;
    render(<Legal settings={s} />);
    const notice = screen.getByTestId("legal-notice").textContent ?? "";
    for (const v of ["株式会社中央総合", "山田太郎", "銀行振込", "8日以内"]) expect(notice).toContain(v);
    expect(screen.getByText("配送のみ")).toBeTruthy();
  });
});
