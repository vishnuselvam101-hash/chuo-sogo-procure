import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Tauri serves the built files; dev server is local only.
// `--mode admin` builds the admin console into dist-admin, `--mode shop` the consumer shop
// into dist-shop.
const KINDS: Record<string, { kind: string; port: number; out: string }> = {
  admin: { kind: "admin", port: 1421, out: "dist-admin" },
  shop: { kind: "shop", port: 1422, out: "dist-shop" },
};
const BUSINESS = { kind: "", port: 1420, out: "dist" };

export default defineConfig(({ mode }) => {
  const app = KINDS[mode] ?? BUSINESS;
  return {
    plugins: [react()],
    clearScreen: false,
    // Build-time constant: each build only contains its own app.
    define: { "import.meta.env.VITE_APP_KIND": JSON.stringify(app.kind) },
    server: { host: "127.0.0.1", port: app.port, strictPort: true },
    build: { target: "es2022", sourcemap: false, outDir: app.out },
    test: {
      environment: "jsdom",
      globals: true,
      include: ["tests/**/*.test.ts", "tests/**/*.test.tsx"],
    },
  };
});
