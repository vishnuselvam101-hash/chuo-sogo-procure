import i18n from "i18next";
import { initReactI18next } from "react-i18next";

/** Languages offered in the Language menu, shown in their own script. */
export const LANGUAGES = [
  { code: "ja", nativeName: "日本語", dir: "ltr" },
  { code: "en", nativeName: "English", dir: "ltr" },
  { code: "ta", nativeName: "தமிழ்", dir: "ltr" },
  { code: "hi", nativeName: "हिन्दी", dir: "ltr" },
  { code: "zh-Hans", nativeName: "简体中文", dir: "ltr" },
  { code: "ko", nativeName: "한국어", dir: "ltr" },
] as const;

export type LanguageCode = (typeof LANGUAGES)[number]["code"];
export const NAMESPACES = [
  "common",
  "procurement",
  "settings",
  "errors",
  "admin",
  "billing",
  "catalogue",
  "history",
  "records",
] as const;
export const DEFAULT_LANGUAGE: LanguageCode = "ja";
export const FALLBACK_LANGUAGE: LanguageCode = "en";

export function isLanguage(code: string | null | undefined): code is LanguageCode {
  return LANGUAGES.some((l) => l.code === code);
}

// All translations are bundled at build time; nothing is fetched at runtime.
const files = import.meta.glob<Record<string, unknown>>("../../locales/*/*.json", {
  eager: true,
  import: "default",
});

export function buildResources(): Record<string, Record<string, Record<string, unknown>>> {
  const resources: Record<string, Record<string, Record<string, unknown>>> = {};
  for (const [path, content] of Object.entries(files)) {
    const match = /locales\/([^/]+)\/([^/]+)\.json$/.exec(path);
    if (!match?.[1] || !match[2]) continue;
    const [, lang, ns] = match;
    (resources[lang] ??= {})[ns] = content;
  }
  return resources;
}

export function setDocumentLanguage(code: LanguageCode): void {
  const lang = LANGUAGES.find((l) => l.code === code);
  document.documentElement.lang = code;
  document.documentElement.dir = lang?.dir ?? "ltr";
}

export async function initI18n(initial: LanguageCode = DEFAULT_LANGUAGE): Promise<typeof i18n> {
  if (!i18n.isInitialized) {
    await i18n.use(initReactI18next).init({
      resources: buildResources(),
      lng: initial,
      fallbackLng: FALLBACK_LANGUAGE,
      ns: [...NAMESPACES],
      defaultNS: "common",
      interpolation: { escapeValue: false }, // React escapes output
      returnNull: false,
    });
  } else {
    await i18n.changeLanguage(initial);
  }
  setDocumentLanguage(initial);
  return i18n;
}

export default i18n;
