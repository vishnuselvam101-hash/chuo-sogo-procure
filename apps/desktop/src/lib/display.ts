import type { Prefs, TextSize } from "./types";

export const TEXT_SIZES: readonly TextSize[] = ["normal", "large", "xlarge"];
export const DEFAULT_TEXT_SIZE: TextSize = "normal";

/** Reading comfort lives on <html> so every app (business, admin, shop) picks it up from CSS. */
export function applyDisplay(prefs: Pick<Prefs, "text_size" | "high_contrast"> | null | undefined): void {
  const root = document.documentElement;
  const size = prefs?.text_size && TEXT_SIZES.includes(prefs.text_size) ? prefs.text_size : DEFAULT_TEXT_SIZE;
  root.dataset.textSize = size;
  if (prefs?.high_contrast) root.dataset.contrast = "high";
  else delete root.dataset.contrast;
}
