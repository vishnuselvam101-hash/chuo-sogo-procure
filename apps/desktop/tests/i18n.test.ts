import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { buildResources, initI18n, LANGUAGES, NAMESPACES } from "../src/i18n";

const resources = buildResources();
const FULL = ["ja", "en", "ta", "hi"] as const;

function keys(obj: unknown, prefix = ""): string[] {
  if (typeof obj !== "object" || obj === null) return [prefix];
  return Object.entries(obj).flatMap(([k, v]) => keys(v, prefix ? `${prefix}.${k}` : k));
}

describe("translation resources", () => {
  it("ships every menu language", () => {
    for (const l of LANGUAGES) expect(resources[l.code], l.code).toBeDefined();
  });

  it("has identical keys in ja, en, ta and hi for every namespace", () => {
    for (const ns of NAMESPACES) {
      const reference = keys(resources.en?.[ns]).sort();
      expect(reference.length, ns).toBeGreaterThan(0);
      for (const lang of FULL) {
        expect(keys(resources[lang]?.[ns]).sort(), `${lang}/${ns}`).toEqual(reference);
      }
    }
  });

  it("future languages only contain keys that exist in English", () => {
    const en = new Set(keys(resources.en?.common));
    for (const lang of ["zh-Hans", "ko"]) {
      for (const k of keys(resources[lang]?.common)) expect(en.has(k), `${lang}:${k}`).toBe(true);
    }
  });

  it("falls back to English for missing translations", async () => {
    const i18n = await initI18n("ko");
    expect(i18n.t("nav.dashboard")).toBe("대시보드");
    expect(i18n.t("nav.suppliers")).toBe("Suppliers");
    expect(i18n.t("errors:forbidden")).toBe("You do not have permission to do this.");
  });

  it("renders Tamil and Hindi script, not escaped or missing text", async () => {
    const i18n = await initI18n("ta");
    expect(i18n.t("nav.approvals")).toBe("ஒப்புதல்கள்");
    await i18n.changeLanguage("hi");
    expect(i18n.t("nav.approvals")).toBe("अनुमोदन");
  });

  it("every translation key used in the source exists in English", () => {
    const src = join(import.meta.dirname, "../src");
    const files: string[] = [];
    const walk = (d: string) =>
      readdirSync(d, { withFileTypes: true }).forEach((e) =>
        e.isDirectory() ? walk(join(d, e.name)) : /\.tsx?$/.test(e.name) && files.push(join(d, e.name)),
      );
    walk(src);
    const used = new Set<string>();
    for (const f of files) {
      for (const m of readFileSync(f, "utf8").matchAll(/\bt\(\s*"([a-zA-Z]+:)?([a-zA-Z][\w.]*)"/g)) {
        used.add(`${m[1] ? m[1].slice(0, -1) : "common"}:${m[2]}`);
      }
    }
    expect(used.size).toBeGreaterThan(20);
    for (const key of used) {
      const [ns, path] = key.split(":") as [string, string];
      const value = path
        .split(".")
        .reduce<unknown>((o, k) => (o as Record<string, unknown>)?.[k], resources.en?.[ns]);
      expect(typeof value, key).toBe("string");
    }
  });
});
