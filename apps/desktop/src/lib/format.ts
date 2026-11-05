// Display-only formatting. Values are never modified or rounded for storage here;
// the backend's integer minor units stay authoritative.

const MINOR_DIGITS: Record<string, number> = { JPY: 0, KRW: 0, USD: 2, EUR: 2, INR: 2, CNY: 2 };

/** Intl locale for a UI language (Tamil/Hindi use Indian conventions). */
export function intlLocale(lang: string): string {
  switch (lang) {
    case "ja":
      return "ja-JP";
    case "ta":
      return "ta-IN";
    case "hi":
      return "hi-IN";
    case "zh-Hans":
      return "zh-Hans-CN";
    case "ko":
      return "ko-KR";
    default:
      return "en-GB";
  }
}

export function formatMoney(minor: number, currency: string, lang: string): string {
  const digits = MINOR_DIGITS[currency] ?? 2;
  return new Intl.NumberFormat(intlLocale(lang), {
    style: "currency",
    currency,
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  }).format(minor / 10 ** digits);
}

export function formatDateTime(iso: string, lang: string, timeZone: string): string {
  return new Intl.DateTimeFormat(intlLocale(lang), {
    dateStyle: "medium",
    timeStyle: "short",
    timeZone,
  }).format(new Date(iso));
}

export function formatNumber(n: number, lang: string): string {
  return new Intl.NumberFormat(intlLocale(lang)).format(n);
}

export function minorDigits(currency: string): number {
  return MINOR_DIGITS[currency] ?? 2;
}

/**
 * Parses a user-typed amount ("1234.5", "1,234") into integer minor units without floating
 * point. Returns null for anything that is not a plain non-negative amount with at most the
 * currency's minor digits.
 */
export function parseMinor(input: string, currency: string): number | null {
  const digits = minorDigits(currency);
  const s = input.trim().replace(/[,\s]/g, "");
  const m = /^(\d+)(?:\.(\d*))?$/.exec(s);
  if (!m?.[1]) return null;
  const frac = m[2] ?? "";
  if (frac.length > digits) return null;
  const minor = BigInt(m[1]) * 10n ** BigInt(digits) + BigInt(frac.padEnd(digits, "0") || "0");
  if (minor > BigInt(Number.MAX_SAFE_INTEGER)) return null;
  return Number(minor);
}

/** Minor units back to a plain editable string ("1234.50"). */
export function minorToInput(minor: number, currency: string): string {
  const digits = minorDigits(currency);
  if (digits === 0) return String(minor);
  const s = String(minor).padStart(digits + 1, "0");
  return `${s.slice(0, -digits)}.${s.slice(-digits)}`;
}

/**
 * A unit price kept in thousandths of a minor unit (purchase history: ¥0.85 = 850), shown with
 * the currency's own decimals plus up to three more when the price has them.
 */
export function formatMilliPrice(milli: number, currency: string, lang: string): string {
  const digits = minorDigits(currency);
  return new Intl.NumberFormat(intlLocale(lang), {
    style: "currency",
    currency,
    minimumFractionDigits: digits,
    maximumFractionDigits: digits + 3,
  }).format(milli / 10 ** (digits + 3));
}

/** A quantity kept in thousandths of a unit (2.5 m = 2500). */
export function formatMilliQuantity(milli: number, lang: string): string {
  return new Intl.NumberFormat(intlLocale(lang), { maximumFractionDigits: 3 }).format(milli / 1000);
}

/** A percentage typed by the user ("10", "12.5") as basis points, exactly; null if invalid. */
export function percentToBps(input: string): number | null {
  const m = /^(\d{1,4})(?:\.(\d{0,2}))?$/.exec(input.trim());
  if (!m?.[1]) return null;
  return Number(m[1]) * 100 + Number((m[2] ?? "").padEnd(2, "0"));
}

export function bpsToPercent(bps: number): string {
  const whole = Math.floor(bps / 100);
  const frac = String(bps % 100)
    .padStart(2, "0")
    .replace(/0+$/, "");
  return frac ? `${whole}.${frac}` : String(whole);
}

export function formatDate(iso: string, lang: string): string {
  return new Intl.DateTimeFormat(intlLocale(lang), { dateStyle: "medium", timeZone: "UTC" }).format(
    new Date(`${iso}T00:00:00Z`),
  );
}

/**
 * A plain non-negative decimal ("0.25", "1,200.5") scaled to an integer with `digits`
 * decimals (0.25 with 3 → 250), exactly; null if it has more decimals or is not a number.
 */
export function parseScaled(input: string, digits: number): number | null {
  const m = /^(\d+)(?:\.(\d*))?$/.exec(input.trim().replace(/[,\s]/g, ""));
  if (!m?.[1]) return null;
  const frac = m[2] ?? "";
  if (frac.length > digits) return null;
  const v = BigInt(m[1]) * 10n ** BigInt(digits) + BigInt(frac.padEnd(digits, "0") || "0");
  return v > BigInt(Number.MAX_SAFE_INTEGER) ? null : Number(v);
}
