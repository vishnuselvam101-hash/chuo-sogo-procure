// The consumer's cart: product ids, names and prices as last seen, and quantities.
// Presentation only — the server recomputes every price and total when the order is
// placed and refuses it if anything changed (`price_changed`).

export interface CartLine {
  listing_id: string;
  title: string;
  model_number: string;
  /// Tax-inclusive unit price in yen, as shown when added.
  unit_price_minor: number;
  quantity: number;
}

const KEY = "procure.shop.cart";

/** Cart for one consumer account; empty when storage is unavailable or unreadable. */
export function loadCart(orgId: string): CartLine[] {
  try {
    const raw = localStorage.getItem(`${KEY}.${orgId}`);
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed)
      ? parsed.filter(
          (l): l is CartLine =>
            typeof l === "object" &&
            l !== null &&
            typeof (l as CartLine).listing_id === "string" &&
            Number.isInteger((l as CartLine).quantity) &&
            (l as CartLine).quantity > 0,
        )
      : [];
  } catch {
    return [];
  }
}

export function saveCart(orgId: string, lines: CartLine[]): void {
  try {
    localStorage.setItem(`${KEY}.${orgId}`, JSON.stringify(lines));
  } catch {
    // Storage full or disabled: the cart simply is not kept between sessions.
  }
}

export function addToCart(lines: CartLine[], line: CartLine): CartLine[] {
  const existing = lines.find((l) => l.listing_id === line.listing_id);
  if (existing) {
    return lines.map((l) =>
      l.listing_id === line.listing_id
        ? {
            ...l,
            quantity: Math.min(1000, l.quantity + line.quantity),
            unit_price_minor: line.unit_price_minor,
          }
        : l,
    );
  }
  return [...lines, line].slice(0, 50);
}

/** What the consumer expects to pay (sent with the order; the server must agree). */
export function expectedTotal(
  lines: CartLine[],
  shippingFee: number,
  freeOver: number | null | undefined,
): { items: number; shipping: number; total: number } {
  const items = lines.reduce((sum, l) => sum + l.unit_price_minor * l.quantity, 0);
  const shipping = freeOver !== null && freeOver !== undefined && items >= freeOver ? 0 : shippingFee;
  return { items, shipping, total: items + shipping };
}
