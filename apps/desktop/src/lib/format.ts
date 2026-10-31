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
